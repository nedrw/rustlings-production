//! probe = "delivery/ordering@apply"
//! title = "Order per key, concurrency everywhere else"
//! hint = "The in-flight map is a write: the caller that finds a key free owns it, and everyone else queues behind the worker that owns it."
//!
//! Step `relay/2.9-ordering` answers two requirements at once, and the second one is the one that
//! breaks in production: a subscriber must see their deliveries in the order they were accepted,
//! and a slow subscriber must not become everybody's problem.
//!
//! 1. **one worker per key, and the key is the subscriber.** A queue per key holds its deliveries
//!    in arrival order, and a key is handed to exactly one worker at a time.
//!    [`Sequencer::submit`] decides *and writes* that in one critical section: the caller that
//!    finds the key free becomes its worker ([`Role::Run`]), and every other caller is told the
//!    job waits ([`Role::Queued`]). A caller that looked at the map and then wrote it would be the
//!    second worker on one key — which is how a subscriber gets two deliveries at once and loses
//!    the order they were accepted in;
//! 2. **different keys do not wait for each other.** One worker for the whole queue, or one global
//!    lock, gets rule 1 right and turns a single slow subscriber into an outage for everyone. A key
//!    with a worker in flight has nothing to do with a key that has none.
//!
//! Order comes out of rule 1 plus the queue — not out of a clock and not out of a counter:
//! [`Sequencer::finished`] hands the owner the next job for the key it holds, so one key's jobs
//! complete in the order they were submitted.
//!
//! ## What you write
//!
//! * [`Sequencer::submit`] — own the key if nobody does, and put the job behind the ones already
//!   running for that key otherwise;
//! * [`Sequencer::finished`] — the owner is done with a job: the next job for that key, or `None`
//!   when the key's queue ran dry, at which point the key is free for the next caller that arrives.
//!
//! [`Sequencer::in_flight`] is given, so the tests can read the map back; it is also the number a
//! dashboard puts next to the backlog.
//!
//! ## What the tests ask
//!
//! The worker loop is `drain` in the test module, and it uses nothing but the two functions above:
//! run what you own, ask for the next one, stop when there is none. So the tests are about *when* a
//! key is owned — five jobs queued behind a worker the test has stopped inside its first job with a
//! `Condvar` gate, and a second key that has to go through anyway — and never about sleeping for a
//! while and hoping. Two of them race eight callers on one key through a `Barrier`.
//!
//! ## Where this is honest, and where it is not
//!
//! A probe has no subscriber and no `tokio`, so a thread is a worker and the gate belongs to the
//! test: the real step hands tasks to a runtime, and what this re-checks is the rule the delivery
//! loop keeps in its own process. Deliberately missing: fairness between keys (a key with a long
//! queue keeps its worker busy while a fresh key gets a fresh worker), a bound on how many workers
//! exist, and what happens to a key's queue when the process dies — that last one is
//! `async/graceful-shutdown`, and a fake version of it would be worse than no probe at all.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

/// One delivery to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The id the queue gave it, which is what a completion log shows.
    pub id: u64,
    /// The subscriber or endpoint this delivery is for — the key everything is serialized on.
    pub key: String,
}

impl Job {
    /// A job for one key.
    #[must_use]
    pub fn new(id: u64, key: &str) -> Self {
        Self {
            id,
            key: key.to_owned(),
        }
    }
}

/// What one call to [`Sequencer::submit`] told its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// Nobody was working this key, so this caller owns it: run this job, then ask
    /// [`Sequencer::finished`] for the next one and keep going until it answers `None`.
    Run(Job),
    /// Somebody else owns this key and will run the job. This caller must not, and there is
    /// nothing for it to do: the job is in the owner's queue.
    Queued,
}

/// What a worker does with one job.
///
/// Injected, because a probe has no subscriber — and because the test's executor is where the order
/// jobs finished in is written down. It answers nothing: whether the delivery worked is the retry
/// and dead-letter steps' business, and for ordering a job that ran is a job that is done.
pub trait Executor {
    /// Make one delivery.
    fn execute(&mut self, job: &Job);
}

/// What is waiting for what, and which keys a worker is draining right now.
#[derive(Debug, Default)]
struct State {
    queues: BTreeMap<String, VecDeque<Job>>,
    in_flight: BTreeSet<String>,
}

/// Hands out one key's jobs to one worker at a time.
#[derive(Debug, Default)]
pub struct Sequencer {
    state: Mutex<State>,
}

impl Sequencer {
    /// An empty sequencer: nothing in flight, nothing waiting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hand `job` to this caller, or queue it behind the jobs its key is already running.
    ///
    /// This is the whole ordering rule, and it is one write: whoever finds the key free owns it
    /// until [`Sequencer::finished`] says its queue ran dry.
    #[must_use]
    pub fn submit(&self, job: Job) -> Role {
        let mut state = self.state.lock().expect("the sequencer is not poisoned");

        // One critical section: the caller that finds the key free takes it, and reserving it
        // happens before anybody else can look. A read followed by a write is two workers on one
        // key, which is two deliveries to one subscriber at the same moment.
        if state.in_flight.contains(&job.key) {
            state
                .queues
                .entry(job.key.clone())
                .or_default()
                .push_back(job);
            return Role::Queued;
        }

        state.in_flight.insert(job.key.clone());
        Role::Run(job)
    }

    /// The worker that owns this job's key is done with it: the next job for that key, or `None`
    /// when nothing is waiting behind it — and then the key is free again.
    ///
    /// Only the owner may call this. A worker that asks about a key it does not hold is asking to
    /// run somebody else's queue, which is the bug this type exists to make impossible to get away
    /// with quietly.
    #[must_use]
    pub fn finished(&self, job: &Job) -> Option<Job> {
        let mut state = self.state.lock().expect("the sequencer is not poisoned");

        // Arrival order is the queue: the next job for this key is the one that waited longest,
        // and there is no other order to choose from.
        if let Some(next) = state.queues.get_mut(&job.key).and_then(VecDeque::pop_front) {
            return Some(next);
        }

        // Nothing is waiting behind it, so the key stops being in flight — and the next caller for
        // it becomes its worker instead of waiting for a worker that already stopped.
        state.queues.remove(&job.key);
        state.in_flight.remove(&job.key);
        None
    }

    /// How many keys have a worker on them right now.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.state
            .lock()
            .expect("the sequencer is not poisoned")
            .in_flight
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc, Barrier, Condvar};
    use std::thread;

    /// How many callers race for one key in the ownership test.
    const CALLERS: usize = 8;
    /// How many jobs each racing worker submits in the exactly-once test.
    const JOBS_PER_WORKER: u64 = 20;

    /// Run jobs the way a worker does: the first one came out of `submit`, every next one out of
    /// `finished`, so this is the loop the two functions have to make correct.
    fn drain(sequencer: &Sequencer, executor: &mut dyn Executor, first: Role) {
        let Role::Run(mut job) = first else {
            return;
        };
        loop {
            executor.execute(&job);
            let Some(next) = sequencer.finished(&job) else {
                return;
            };
            job = next;
        }
    }

    /// A gate the test opens when it wants to. A job waits on it, which is how a test holds a
    /// worker inside one key without sleeping.
    #[derive(Debug, Default)]
    struct Gate {
        open: Mutex<bool>,
        opened: Condvar,
    }

    impl Gate {
        fn wait(&self) {
            let mut open = self.open.lock().expect("the gate is not poisoned");
            while !*open {
                open = self.opened.wait(open).expect("the gate is not poisoned");
            }
        }

        fn open(&self) {
            *self.open.lock().expect("the gate is not poisoned") = true;
            self.opened.notify_all();
        }
    }

    /// Records what finished, in the order it finished, and how many jobs of one key were ever
    /// inside `execute` at the same time.
    #[derive(Debug, Default)]
    struct Recorder {
        log: Mutex<Vec<Job>>,
        live: Mutex<BTreeMap<String, usize>>,
        peak: Mutex<BTreeMap<String, usize>>,
    }

    impl Recorder {
        /// Every job that finished, oldest first.
        fn log(&self) -> Vec<Job> {
            self.log.lock().expect("the log is not poisoned").clone()
        }

        /// The most jobs of this key that were ever running at once.
        fn peak(&self, key: &str) -> usize {
            self.peak
                .lock()
                .expect("the peak map is not poisoned")
                .get(key)
                .copied()
                .unwrap_or(0)
        }

        fn record(&self, job: &Job) {
            let running = {
                let mut live = self.live.lock().expect("the live map is not poisoned");
                let count = live.entry(job.key.clone()).or_insert(0);
                *count += 1;
                *count
            };
            let mut peak = self.peak.lock().expect("the peak map is not poisoned");
            let highest = peak.entry(job.key.clone()).or_insert(0);
            *highest = (*highest).max(running);
            drop(peak);

            // Step aside, so a second worker on the same key would be *inside* this window rather
            // than waiting for it: a worker that ran jobs back to back would hide a missing
            // reservation.
            thread::yield_now();

            self.log
                .lock()
                .expect("the log is not poisoned")
                .push(job.clone());
            let mut live = self.live.lock().expect("the live map is not poisoned");
            *live.get_mut(&job.key).expect("counted above") -= 1;
        }
    }

    impl Executor for Recorder {
        fn execute(&mut self, job: &Job) {
            self.record(job);
        }
    }

    /// One worker's view of a recorder all the workers write to.
    struct Shared(Arc<Recorder>);

    impl Executor for Shared {
        fn execute(&mut self, job: &Job) {
            self.0.record(job);
        }
    }

    /// Blocks the first job of one key until the test opens the gate, and says when it got there.
    struct Blocking {
        key: String,
        gate: Arc<Gate>,
        started: Sender<Job>,
        inner: Recorder,
    }

    impl Executor for Blocking {
        fn execute(&mut self, job: &Job) {
            if job.key == self.key {
                let _ = self.started.send(job.clone());
                self.gate.wait();
            }
            self.inner.record(job);
        }
    }

    fn ids(jobs: &[Job]) -> Vec<u64> {
        jobs.iter().map(|job| job.id).collect()
    }

    #[test]
    fn one_caller_owns_a_key_and_the_rest_wait_behind_it() {
        let sequencer = Sequencer::new();

        let first = sequencer.submit(Job::new(1, "ada"));
        assert_eq!(
            first,
            Role::Run(Job::new(1, "ada")),
            "the first job of a free key is handed to its caller"
        );
        assert_eq!(
            sequencer.submit(Job::new(2, "ada")),
            Role::Queued,
            "a job for a key that is in flight waits: it must not be run beside the job that owns \
             the key"
        );
        assert_eq!(sequencer.submit(Job::new(3, "ada")), Role::Queued);
        assert_eq!(
            sequencer.submit(Job::new(4, "grace")),
            Role::Run(Job::new(4, "grace")),
            "a different key is a different question: `ada` being busy says nothing about `grace`"
        );
        assert_eq!(sequencer.in_flight(), 2);

        // The owner finishes: the next job of its key, in arrival order.
        assert_eq!(
            sequencer.finished(&Job::new(1, "ada")),
            Some(Job::new(2, "ada"))
        );
        assert_eq!(
            sequencer.in_flight(),
            2,
            "the key is still owned while its queue has something in it"
        );
        assert_eq!(
            sequencer.finished(&Job::new(2, "ada")),
            Some(Job::new(3, "ada"))
        );
        assert_eq!(
            sequencer.finished(&Job::new(3, "ada")),
            None,
            "nothing is left behind the job that just finished"
        );
        assert_eq!(
            sequencer.in_flight(),
            1,
            "`ada` is free again, `grace` is not"
        );

        // And the next caller for that key gets it, rather than waiting for a worker that stopped.
        assert_eq!(
            sequencer.submit(Job::new(5, "ada")),
            Role::Run(Job::new(5, "ada")),
            "an idle key is owned by whoever arrives next"
        );
    }

    #[test]
    fn a_key_is_drained_in_the_order_its_jobs_were_queued() {
        let sequencer = Arc::new(Sequencer::new());
        let gate = Arc::new(Gate::default());
        let (started_tx, started) = mpsc::channel();

        // The queue is built by one thread, so the order it holds is the order these arrived in:
        // 1 owns the key, 2..=5 wait behind it.
        let first = sequencer.submit(Job::new(1, "ada"));
        for id in 2..=5 {
            assert_eq!(
                sequencer.submit(Job::new(id, "ada")),
                Role::Queued,
                "job {id} arrives while the key is in flight, so it waits instead of overtaking"
            );
        }

        let worker = Arc::clone(&sequencer);
        let worker_gate = Arc::clone(&gate);
        let handle = thread::spawn(move || {
            let mut executor = Blocking {
                key: "ada".to_owned(),
                gate: worker_gate,
                started: started_tx,
                inner: Recorder::default(),
            };
            drain(&worker, &mut executor, first);
            executor
        });

        // The worker is inside job 1 and cannot leave it, so everything below provably happens
        // while 1 is still running.
        let running = started
            .recv()
            .unwrap_or_else(|err| panic!("the worker never started job 1: {err}"));
        assert_eq!(
            running.id, 1,
            "the owner started with the job it was handed"
        );
        assert_eq!(
            sequencer.submit(Job::new(6, "ada")),
            Role::Queued,
            "a caller cannot take over a key that is already in flight"
        );
        assert_eq!(sequencer.in_flight(), 1, "one busy key, one worker");

        gate.open();
        let executor = handle
            .join()
            .unwrap_or_else(|_| panic!("the worker panicked while draining"));
        assert_eq!(
            ids(&executor.inner.log()),
            vec![1, 2, 3, 4, 5, 6],
            "one key is drained in the order its jobs were queued, including the one that arrived \
             while job 1 was still running"
        );
        assert_eq!(
            executor.inner.peak("ada"),
            1,
            "no two workers were ever inside this key at the same time"
        );
        assert_eq!(
            sequencer.in_flight(),
            0,
            "the key is free once its queue ran dry"
        );
    }

    #[test]
    fn a_slow_key_does_not_hold_up_another() {
        let sequencer = Arc::new(Sequencer::new());
        let gate = Arc::new(Gate::default());
        let (started_tx, started) = mpsc::channel();

        // `slow` owns its key and is stopped inside its first job, with a second job behind it.
        let slow = sequencer.submit(Job::new(1, "slow"));
        assert_eq!(sequencer.submit(Job::new(2, "slow")), Role::Queued);

        let worker = Arc::clone(&sequencer);
        let worker_gate = Arc::clone(&gate);
        let handle = thread::spawn(move || {
            let mut executor = Blocking {
                key: "slow".to_owned(),
                gate: worker_gate,
                started: started_tx,
                inner: Recorder::default(),
            };
            drain(&worker, &mut executor, slow);
            executor
        });
        let running = started
            .recv()
            .unwrap_or_else(|err| panic!("the worker never started the slow job: {err}"));
        assert_eq!(running.id, 1);

        // While that one is stuck, another key's delivery goes through on this thread — and both
        // keys have a worker, which is the difference between ordering and a global lock.
        let mut fast = Recorder::default();
        let role = sequencer.submit(Job::new(3, "fast"));
        assert_eq!(
            sequencer.in_flight(),
            2,
            "two keys have a worker on them at once: one slow subscriber is not everybody's delay"
        );
        drain(&sequencer, &mut fast, role);
        assert_eq!(
            ids(&fast.log()),
            vec![3],
            "a second endpoint's delivery was made while the first one was still in flight"
        );
        assert_eq!(
            sequencer.in_flight(),
            1,
            "the fast key came free again; the slow one is still busy"
        );

        gate.open();
        let slow_executor = handle
            .join()
            .unwrap_or_else(|_| panic!("the worker panicked while draining"));
        assert_eq!(
            ids(&slow_executor.inner.log()),
            vec![1, 2],
            "and the slow key's own jobs still ran one after the other, in order"
        );
        assert_eq!(sequencer.in_flight(), 0);
    }

    #[test]
    fn eight_callers_racing_for_one_key_leave_exactly_one_owner() {
        let sequencer = Arc::new(Sequencer::new());
        let barrier = Arc::new(Barrier::new(CALLERS));
        let handles: Vec<_> = (0..CALLERS)
            .map(|index| {
                let sequencer = Arc::clone(&sequencer);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    sequencer.submit(Job::new(index as u64, "ada"))
                })
            })
            .collect();
        let roles: Vec<Role> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| panic!("a caller panicked while submitting"))
            })
            .collect();

        assert_eq!(
            roles
                .iter()
                .filter(|role| matches!(role, Role::Run(_)))
                .count(),
            1,
            "exactly one of {CALLERS} simultaneous submits owns the key — the decision is one \
             write, not a read followed by one: {roles:?}"
        );
        assert_eq!(
            roles
                .iter()
                .filter(|role| matches!(role, Role::Queued))
                .count(),
            CALLERS - 1,
            "every other caller is told the job waits: {roles:?}"
        );
        assert_eq!(sequencer.in_flight(), 1);

        // Keys that differ are not competitors: every caller owns its own key.
        let sequencer = Arc::new(Sequencer::new());
        let barrier = Arc::new(Barrier::new(CALLERS));
        let handles: Vec<_> = (0..CALLERS)
            .map(|index| {
                let sequencer = Arc::clone(&sequencer);
                let barrier = Arc::clone(&barrier);
                let key = format!("subscriber-{index}");
                thread::spawn(move || {
                    barrier.wait();
                    sequencer.submit(Job::new(index as u64, &key))
                })
            })
            .collect();
        let roles: Vec<Role> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| panic!("a caller panicked while submitting"))
            })
            .collect();
        assert!(
            roles.iter().all(|role| matches!(role, Role::Run(_))),
            "keys that differ do not queue behind each other: {roles:?}"
        );
        assert_eq!(sequencer.in_flight(), CALLERS);
    }

    #[test]
    fn every_job_of_a_key_runs_exactly_once_under_threads() {
        let sequencer = Arc::new(Sequencer::new());
        let recorder = Arc::new(Recorder::default());

        let mut handles = Vec::new();
        for worker in 0..CALLERS {
            let sequencer = Arc::clone(&sequencer);
            let recorder = Arc::clone(&recorder);
            handles.push(thread::spawn(move || {
                let mut executor = Shared(recorder);
                for index in 0..JOBS_PER_WORKER {
                    let id = worker as u64 * JOBS_PER_WORKER + index;
                    // A caller that owns the key drains it, including the jobs other callers queued
                    // behind it; a caller that is told the job waits has nothing left to do, because
                    // whoever owns the key will find it.
                    if let Role::Run(job) = sequencer.submit(Job::new(id, "ada")) {
                        drain(&sequencer, &mut executor, Role::Run(job));
                    }
                }
            }));
        }
        for handle in handles {
            handle
                .join()
                .unwrap_or_else(|_| panic!("a worker panicked while submitting"));
        }

        let log = recorder.log();
        let ran = ids(&log);
        assert_eq!(
            ran.len(),
            (CALLERS as u64 * JOBS_PER_WORKER) as usize,
            "every job ran: a queued job nobody picked up is a delivery that never happens"
        );
        let mut sorted = ran;
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            (0..CALLERS as u64 * JOBS_PER_WORKER).collect::<Vec<u64>>(),
            "and none of them ran twice"
        );
        assert_eq!(
            recorder.peak("ada"),
            1,
            "however the callers interleaved, one key had one worker inside it: {:?}",
            log.len()
        );
        assert_eq!(sequencer.in_flight(), 0, "every key came free again");
    }
}
