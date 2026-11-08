//! How long a run has left, from how long the same workflow took before.
//!
//! A port of gh-eta's estimator (`estimate.go`, see its `docs/how-it-works.md`
//! for the model and measured accuracy). Whole-run duration is a poor baseline:
//! it includes queueing and approval waits. So this works per job and per step,
//! and learns the order jobs run in from history, so jobs running in parallel
//! overlap rather than add up.
//!
//! Pure: `now` is always an argument. The tests are gh-eta's.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, TimeDelta, Utc};

use super::{CiJob, CiRun, RunStatus};

/// How many past successful runs a workflow's history learns from.
pub const HISTORY_RUNS: usize = 10;

/// A summary of a set of observed durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub p50: TimeDelta,
    pub p90: TimeDelta,
    pub n: usize,
}

/// What past runs say about one job, keyed by job name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobHistory {
    pub duration: Stats,
    /// The delay between the job's predecessors finishing (or the run starting,
    /// for jobs with no predecessors) and the job starting. Covers runner
    /// pickup and, for gated jobs, the usual approval wait.
    pub gap: Stats,
    pub steps: HashMap<String, Stats>,
    /// Jobs that finished before this one started in every past run where both
    /// ran. They stand in for the workflow's `needs` graph.
    pub preds: HashSet<String>,
}

/// The per-job summary of a set of successful past runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct History {
    pub runs: usize,
    pub jobs: HashMap<String, JobHistory>,
}

/// What the forecast says about one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Done,
    Running,
    /// Past its P90, with nothing in its history saying how much longer.
    Overdue,
    Pending,
    /// Waiting on an environment approval, or behind a job that is.
    Blocked,
    /// Has never succeeded before, so it is left out of the total.
    Unknown,
}

/// How a job's remaining time was worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    Steps,
    Job,
}

/// The forecast for one job of the run being estimated, by position in
/// `run.jobs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEstimate {
    pub state: JobState,
    pub elapsed: TimeDelta,
    pub remaining: TimeDelta,
    pub basis: Option<Basis>,
}

/// The forecast for a whole run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEstimate {
    pub history_runs: usize,
    pub elapsed: TimeDelta,
    pub remaining: TimeDelta,
    pub eta: DateTime<Utc>,
    /// A job is waiting on an environment approval. `remaining` and `eta` then
    /// assume the approval is given now.
    pub blocked: bool,
    /// Some unfinished job has no history, so `remaining` leaves it out.
    pub partial: bool,
    pub jobs: Vec<JobEstimate>,
}

/// Whether a job or step ran to a successful end, the only kind of sample
/// worth timing.
fn succeeded(status: RunStatus) -> bool {
    status == RunStatus::Success
}

fn new_stats(ds: &[TimeDelta]) -> Stats {
    if ds.is_empty() {
        return Stats::default();
    }
    let mut sorted = ds.to_vec();
    sorted.sort();
    Stats {
        p50: percentile(&sorted, 0.5),
        p90: percentile(&sorted, 0.9),
        n: sorted.len(),
    }
}

/// Interpolates linearly between the closest ranks of `sorted`.
fn percentile(sorted: &[TimeDelta], p: f64) -> TimeDelta {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = p * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    let span = (sorted[hi] - sorted[lo]).num_milliseconds() as f64;
    sorted[lo] + TimeDelta::milliseconds((frac * span) as i64)
}

/// How much longer something typically lasting `st` will run, given it has run
/// for `elapsed`. Once past the median, the P90 is the better guess.
fn left(st: Stats, elapsed: TimeDelta) -> TimeDelta {
    if elapsed < st.p50 {
        st.p50 - elapsed
    } else {
        non_negative(st.p90 - elapsed)
    }
}

fn non_negative(d: TimeDelta) -> TimeDelta {
    d.max(TimeDelta::zero())
}

/// Summarise past runs. Only successful jobs and steps count.
pub fn build_history(runs: &[CiRun]) -> History {
    type Obs = (DateTime<Utc>, DateTime<Utc>);
    let mut per_run: Vec<HashMap<&str, Obs>> = Vec::with_capacity(runs.len());
    let mut durations: HashMap<&str, Vec<TimeDelta>> = HashMap::new();
    let mut steps: HashMap<&str, HashMap<&str, Vec<TimeDelta>>> = HashMap::new();

    for r in runs {
        let mut seen = HashMap::new();
        for j in &r.jobs {
            let (true, Some(start), Some(end)) =
                (succeeded(j.status), j.started_at, j.completed_at)
            else {
                continue;
            };
            seen.insert(j.name.as_str(), (start, end));
            durations
                .entry(&j.name)
                .or_default()
                .push(non_negative(end - start));
            for s in &j.steps {
                let (true, Some(start), Some(end)) =
                    (succeeded(s.status), s.started_at, s.completed_at)
                else {
                    continue;
                };
                steps
                    .entry(&j.name)
                    .or_default()
                    .entry(&s.name)
                    .or_default()
                    .push(non_negative(end - start));
            }
        }
        per_run.push(seen);
    }

    let mut h = History {
        runs: runs.len(),
        jobs: HashMap::new(),
    };
    for (name, ds) in &durations {
        let mut jh = JobHistory {
            duration: new_stats(ds),
            ..JobHistory::default()
        };
        if let Some(job_steps) = steps.get(name) {
            for (step, sds) in job_steps {
                jh.steps.insert((*step).to_string(), new_stats(sds));
            }
        }
        h.jobs.insert((*name).to_string(), jh);
    }

    // A job is a predecessor of another if it always finished first.
    let names: Vec<String> = h.jobs.keys().cloned().collect();
    for b in &names {
        for p in &names {
            if p == b {
                continue;
            }
            let mut together = 0;
            let mut always = true;
            for seen in &per_run {
                let (Some(po), Some(bo)) = (seen.get(p.as_str()), seen.get(b.as_str())) else {
                    continue;
                };
                together += 1;
                if po.1 > bo.0 {
                    always = false;
                    break;
                }
            }
            if together > 0
                && always
                && let Some(jh) = h.jobs.get_mut(b)
            {
                jh.preds.insert(p.clone());
            }
        }
    }

    let mut gaps: HashMap<String, Vec<TimeDelta>> = HashMap::new();
    for (i, seen) in per_run.iter().enumerate() {
        for (b, bo) in seen {
            let mut frontier = runs[i].run_started_at;
            for p in &h.jobs[*b].preds {
                if let Some(po) = seen.get(p.as_str())
                    && Some(po.1) > frontier
                {
                    frontier = Some(po.1);
                }
            }
            if let Some(frontier) = frontier {
                gaps.entry((*b).to_string())
                    .or_default()
                    .push(non_negative(bo.0 - frontier));
            }
        }
    }
    for (name, gs) in gaps {
        if let Some(jh) = h.jobs.get_mut(&name) {
            jh.gap = new_stats(&gs);
        }
    }
    h
}

/// What is left of a running job from its steps' history. `None` when some
/// unfinished step has never been seen before, or the job reports no steps yet.
fn remaining_by_steps(j: &CiJob, jh: &JobHistory, now: DateTime<Utc>) -> Option<TimeDelta> {
    let mut rest = TimeDelta::zero();
    for s in &j.steps {
        if s.status.is_done() {
            continue;
        }
        let st = *jh.steps.get(&s.name)?;
        rest += if s.status == RunStatus::Running {
            left(st, now - s.started_at.unwrap_or(now))
        } else {
            st.p50
        };
    }
    (!j.steps.is_empty()).then_some(rest)
}

/// Forecast when `run` will finish, given the history of its workflow. Jobs are
/// scheduled after their learned predecessors, so parallel jobs overlap rather
/// than add up.
pub fn estimate(run: &CiRun, h: &History, now: DateTime<Utc>) -> RunEstimate {
    let elapsed = run.run_started_at.map_or(TimeDelta::zero(), |start| {
        let end = match run.updated_at {
            Some(updated) if run.status.is_done() => updated,
            _ => now,
        };
        non_negative(end - start)
    });
    let mut partial = false;

    let present: HashSet<&str> = run.jobs.iter().map(|j| j.name.as_str()).collect();
    let mut ends: HashMap<&str, Option<DateTime<Utc>>> = HashMap::new();
    let mut jobs: Vec<JobEstimate> = Vec::with_capacity(run.jobs.len());
    let mut resolved = vec![false; run.jobs.len()];

    // Running and finished jobs need nothing from the others.
    for (i, j) in run.jobs.iter().enumerate() {
        let jh = h.jobs.get(&j.name);
        let mut je = JobEstimate {
            state: JobState::Pending,
            elapsed: TimeDelta::zero(),
            remaining: TimeDelta::zero(),
            basis: None,
        };
        match j.status {
            s if s.is_done() => {
                je.state = JobState::Done;
                if let (Some(start), Some(end)) = (j.started_at, j.completed_at) {
                    je.elapsed = non_negative(end - start);
                }
                ends.insert(&j.name, j.completed_at);
                resolved[i] = true;
            }
            RunStatus::Running => {
                je.elapsed = non_negative(now - j.started_at.unwrap_or(now));
                if let Some(jh) = jh {
                    je.state = JobState::Running;
                    match remaining_by_steps(j, jh, now) {
                        Some(rest) => {
                            je.remaining = rest;
                            je.basis = Some(Basis::Steps);
                        }
                        None => {
                            je.remaining = left(jh.duration, je.elapsed);
                            je.basis = Some(Basis::Job);
                        }
                    }
                    if je.elapsed > jh.duration.p90 && je.remaining.is_zero() {
                        je.state = JobState::Overdue;
                    }
                    ends.insert(&j.name, Some(now + je.remaining));
                } else {
                    je.state = JobState::Unknown;
                    partial = true;
                    ends.insert(&j.name, Some(now));
                }
                resolved[i] = true;
            }
            _ => {}
        }
        jobs.push(je);
    }

    // Queued and waiting jobs start once their predecessors are done. Keep
    // passing over them until nothing changes; anything left over (a cycle,
    // which history should never produce) is scheduled from the run's start.
    let mut blocked_job: HashMap<&str, bool> = HashMap::new();
    let mut progress = true;
    while progress {
        progress = false;
        for (i, j) in run.jobs.iter().enumerate() {
            if resolved[i] {
                continue;
            }
            let Some(jh) = h.jobs.get(&j.name) else {
                jobs[i].state = JobState::Unknown;
                partial = true;
                ends.insert(&j.name, Some(now));
                resolved[i] = true;
                progress = true;
                continue;
            };
            let mut start = run.run_started_at;
            let mut ready = true;
            let mut blocked = j.status == RunStatus::Waiting;
            for p in &jh.preds {
                if !present.contains(p.as_str()) {
                    continue;
                }
                let Some(end) = ends.get(p.as_str()) else {
                    ready = false;
                    break;
                };
                if *end > start {
                    start = *end;
                }
                blocked = blocked || blocked_job.get(p.as_str()).copied().unwrap_or(false);
            }
            if !ready {
                continue;
            }
            schedule_job(&mut jobs[i], jh, j, start, now, blocked);
            blocked_job.insert(&j.name, blocked);
            ends.insert(&j.name, Some(now + jobs[i].remaining));
            resolved[i] = true;
            progress = true;
        }
    }
    for (i, j) in run.jobs.iter().enumerate() {
        if !resolved[i]
            && let Some(jh) = h.jobs.get(&j.name)
        {
            let blocked = j.status == RunStatus::Waiting;
            schedule_job(&mut jobs[i], jh, j, run.run_started_at, now, blocked);
            ends.insert(&j.name, Some(now + jobs[i].remaining));
        }
    }

    let blocked = jobs.iter().any(|je| je.state == JobState::Blocked);
    let last = run
        .jobs
        .iter()
        .zip(&jobs)
        .filter(|(_, je)| je.state != JobState::Done)
        .filter_map(|(j, _)| ends.get(j.name.as_str()).copied().flatten())
        .max();
    let remaining = last
        .filter(|l| *l > now)
        .map_or(TimeDelta::zero(), |l| l - now);

    RunEstimate {
        history_runs: h.runs,
        elapsed,
        remaining,
        eta: now + remaining,
        blocked,
        partial,
        jobs,
    }
}

/// Fill in a not-yet-started job that is due to start once `predecessors_end`
/// has passed. A job waiting on approval (or behind one) is assumed to be
/// approved now, with no further gap.
fn schedule_job(
    je: &mut JobEstimate,
    jh: &JobHistory,
    j: &CiJob,
    predecessors_end: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    blocked: bool,
) {
    je.basis = Some(Basis::Job);
    je.state = if blocked {
        JobState::Blocked
    } else {
        JobState::Pending
    };
    let mut start = predecessors_end.map_or(now, |end| end.max(now));
    if !blocked && j.status != RunStatus::Waiting {
        let gap_end = predecessors_end.map_or(now, |end| end + jh.gap.p50);
        start = start.max(gap_end);
    }
    je.remaining = (start + jh.duration.p50) - now;
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::super::CiStep;
    use super::*;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 17, 4, 0, 0).unwrap()
    }

    fn mins(m: f64) -> TimeDelta {
        TimeDelta::milliseconds((m * 60_000.0) as i64)
    }

    fn at(m: f64) -> DateTime<Utc> {
        t0() + mins(m)
    }

    /// A successful job that ran from `start` to `end` minutes after `t0`.
    fn job(name: &str, start: f64, end: f64, steps: Vec<CiStep>) -> CiJob {
        CiJob {
            id: 0,
            name: name.into(),
            status: RunStatus::Success,
            started_at: Some(at(start)),
            completed_at: Some(at(end)),
            html_url: None,
            steps,
        }
    }

    fn step(name: &str, start: f64, end: f64) -> CiStep {
        CiStep {
            name: name.into(),
            status: RunStatus::Success,
            started_at: Some(at(start)),
            completed_at: Some(at(end)),
        }
    }

    fn live_step(name: &str, status: RunStatus) -> CiStep {
        CiStep {
            name: name.into(),
            status,
            started_at: (status == RunStatus::Running).then(t0),
            completed_at: None,
        }
    }

    fn live_job(name: &str, status: RunStatus, steps: Vec<CiStep>) -> CiJob {
        CiJob {
            id: 0,
            name: name.into(),
            status,
            started_at: (status == RunStatus::Running).then(t0),
            completed_at: None,
            html_url: None,
            steps,
        }
    }

    fn past_run(id: i64, jobs: Vec<CiJob>) -> CiRun {
        CiRun {
            id,
            workflow_id: 1,
            name: Some("CI".into()),
            event: "push".into(),
            status: RunStatus::Success,
            run_number: id as u64,
            run_started_at: Some(t0()),
            jobs,
            ..CiRun::default()
        }
    }

    /// Rewind a finished run to how the API showed it at time `t`.
    fn snapshot_at(r: &CiRun, t: DateTime<Utc>) -> CiRun {
        let mut r = r.clone();
        r.status = RunStatus::Running;
        for j in &mut r.jobs {
            rewind(&mut j.status, &mut j.started_at, &mut j.completed_at, t);
            for s in &mut j.steps {
                rewind(&mut s.status, &mut s.started_at, &mut s.completed_at, t);
            }
        }
        r
    }

    fn rewind(
        status: &mut RunStatus,
        started: &mut Option<DateTime<Utc>>,
        completed: &mut Option<DateTime<Utc>>,
        t: DateTime<Utc>,
    ) {
        if completed.is_some_and(|c| c <= t) {
            return;
        }
        *completed = None;
        if started.is_some_and(|s| s <= t) {
            *status = RunStatus::Running;
        } else {
            *status = RunStatus::Queued;
            *started = None;
        }
    }

    #[track_caller]
    fn approx(what: &str, got: TimeDelta, want: TimeDelta) {
        let diff = (got - want).num_milliseconds().abs();
        assert!(diff <= 1000, "{what} = {got}, want {want}");
    }

    #[test]
    fn percentile_interpolates() {
        let s = new_stats(&[mins(4.0), mins(1.0), mins(3.0), mins(2.0)]);
        approx("p50", s.p50, mins(2.5));
        approx("p90", s.p90, mins(3.7));
        assert_eq!(s.n, 4);
        let one = new_stats(&[mins(1.0)]);
        assert_eq!((one.p50, one.p90), (mins(1.0), mins(1.0)));
        assert_eq!(new_stats(&[]), Stats::default());
    }

    #[test]
    fn history_learns_ordering() {
        let h = build_history(&[
            past_run(
                1,
                vec![
                    job("lint", 0.0, 2.0, vec![]),
                    job("test", 0.0, 5.0, vec![]),
                    job("deploy", 5.5, 6.5, vec![]),
                ],
            ),
            past_run(
                2,
                vec![
                    job("lint", 0.0, 3.0, vec![]),
                    job("test", 0.5, 4.0, vec![]),
                    job("deploy", 4.5, 5.5, vec![]),
                ],
            ),
        ]);
        let deploy = &h.jobs["deploy"];
        assert!(deploy.preds.contains("lint") && deploy.preds.contains("test"));
        // lint and test overlap, so neither waits for the other.
        assert!(h.jobs["lint"].preds.is_empty() && h.jobs["test"].preds.is_empty());
        approx("deploy gap", deploy.gap.p50, mins(0.5));
        approx("deploy duration", deploy.duration.p50, mins(1.0));
    }

    #[test]
    fn history_ignores_failed_and_skipped() {
        let mut skipped = job("sign", 0.0, 0.0, vec![]);
        skipped.status = RunStatus::Skipped;
        let h = build_history(&[past_run(1, vec![job("build", 0.0, 10.0, vec![]), skipped])]);
        assert!(!h.jobs.contains_key("sign"));
    }

    #[test]
    fn sequential_jobs_add_up_with_their_gap() {
        let mk = |id| {
            past_run(
                id,
                vec![
                    job("build", 0.0, 10.0, vec![]),
                    job("sign", 11.0, 12.0, vec![]),
                ],
            )
        };
        let h = build_history(&[mk(1), mk(2)]);
        let e = estimate(&snapshot_at(&mk(3), at(4.0)), &h, at(4.0));
        // 6m of build, 1m gap, 1m sign.
        approx("remaining", e.remaining, mins(8.0));
        assert!(!e.blocked && !e.partial);
        assert_eq!(e.jobs[0].state, JobState::Running);
        assert_eq!(e.jobs[1].state, JobState::Pending);
    }

    #[test]
    fn parallel_jobs_overlap() {
        let mk = |id| {
            past_run(
                id,
                vec![
                    job("lint", 0.0, 2.0, vec![]),
                    job("test", 0.0, 5.0, vec![]),
                    job("deploy", 5.0, 6.0, vec![]),
                ],
            )
        };
        let h = build_history(&[mk(1), mk(2)]);
        let e = estimate(&snapshot_at(&mk(3), at(1.0)), &h, at(1.0));
        // test has 4m left, then deploy 1m. Summing lint would give 6m.
        approx("remaining", e.remaining, mins(5.0));
    }

    #[test]
    fn a_job_waiting_for_approval_blocks_the_run() {
        let h = build_history(&[
            past_run(
                1,
                vec![
                    job("build", 0.0, 10.0, vec![]),
                    job("sign", 30.0, 31.0, vec![]),
                ],
            ),
            past_run(
                2,
                vec![
                    job("build", 0.0, 10.0, vec![]),
                    job("sign", 15.0, 16.0, vec![]),
                ],
            ),
        ]);
        let mut cur = past_run(3, vec![job("build", 0.0, 8.0, vec![])]);
        cur.status = RunStatus::Waiting;
        let mut sign = live_job("sign", RunStatus::Waiting, vec![]);
        sign.started_at = Some(at(8.0));
        cur.jobs.push(sign);
        let e = estimate(&cur, &h, at(12.0));
        assert!(e.blocked);
        assert_eq!(e.jobs[1].state, JobState::Blocked);
        // The usual approval wait is not counted once a job is actually waiting.
        approx("remaining", e.remaining, mins(1.0));
    }

    #[test]
    fn a_running_job_is_estimated_from_its_steps() {
        let mk = |id| {
            past_run(
                id,
                vec![job(
                    "build",
                    0.0,
                    10.0,
                    vec![
                        step("checkout", 0.0, 1.0),
                        step("compile", 1.0, 9.0),
                        step("upload", 9.0, 10.0),
                    ],
                )],
            )
        };
        let h = build_history(&[mk(1), mk(2)]);
        // A slow checkout this time: 3m in and still checking out.
        let cur = past_run(
            3,
            vec![live_job(
                "build",
                RunStatus::Running,
                vec![
                    live_step("checkout", RunStatus::Running),
                    live_step("compile", RunStatus::Queued),
                    live_step("upload", RunStatus::Queued),
                ],
            )],
        );
        let e = estimate(&cur, &h, at(3.0));
        assert_eq!(e.jobs[0].basis, Some(Basis::Steps));
        // Job-level would say 7m; the steps still to run take 9m.
        approx("remaining", e.remaining, mins(9.0));
    }

    #[test]
    fn an_unknown_step_falls_back_to_the_job() {
        let h = build_history(&[past_run(
            1,
            vec![job("build", 0.0, 10.0, vec![step("compile", 0.0, 10.0)])],
        )]);
        let cur = past_run(
            2,
            vec![live_job(
                "build",
                RunStatus::Running,
                vec![live_step("brand new step", RunStatus::Running)],
            )],
        );
        let e = estimate(&cur, &h, at(4.0));
        assert_eq!(e.jobs[0].basis, Some(Basis::Job));
        approx("remaining", e.remaining, mins(6.0));
    }

    #[test]
    fn an_unknown_job_marks_the_estimate_partial() {
        let h = build_history(&[past_run(1, vec![job("build", 0.0, 10.0, vec![])])]);
        let cur = past_run(
            2,
            vec![
                live_job("build", RunStatus::Running, vec![]),
                live_job("new-job", RunStatus::Queued, vec![]),
            ],
        );
        let e = estimate(&cur, &h, at(2.0));
        assert!(e.partial);
        assert_eq!(e.jobs[1].state, JobState::Unknown);
        approx("remaining", e.remaining, mins(8.0));
    }

    #[test]
    fn a_job_past_its_p90_is_overdue() {
        let h = build_history(&[
            past_run(1, vec![job("build", 0.0, 10.0, vec![])]),
            past_run(2, vec![job("build", 0.0, 12.0, vec![])]),
        ]);
        let cur = past_run(3, vec![live_job("build", RunStatus::Running, vec![])]);
        let e = estimate(&cur, &h, at(20.0));
        assert_eq!(e.jobs[0].state, JobState::Overdue);
        assert_eq!(e.remaining, TimeDelta::zero());
    }

    #[test]
    fn replaying_a_repeat_run_is_exact() {
        let mk = |id| {
            past_run(
                id,
                vec![
                    job(
                        "build",
                        0.2,
                        9.0,
                        vec![step("setup", 0.2, 1.0), step("gradle", 1.0, 9.0)],
                    ),
                    job("sign", 9.3, 10.0, vec![step("sign", 9.3, 10.0)]),
                ],
            )
        };
        let h = build_history(&[mk(1), mk(2), mk(3)]);
        for m in [0.5, 3.0, 8.9, 9.1, 9.5] {
            let e = estimate(&snapshot_at(&mk(4), at(m)), &h, at(m));
            approx(
                &format!("ETA error at {m}m"),
                e.eta - at(10.0),
                TimeDelta::zero(),
            );
        }
    }
}
