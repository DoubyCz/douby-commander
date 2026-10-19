use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use dc_i18n::{fill, gettext};

use crate::Answer;
use crate::operation::{Control, Operation, OperationContext, Progress, Question};

/// Queue an operation runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueueId {
    /// Operations start at once and run side by side.
    Free,
    /// Operations run one after another; different queues run side by side.
    Numbered(u32),
}

impl fmt::Display for QueueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Captions of the operations viewer of the original.
            Self::Free => f.write_str(&gettext("Out of queue")),
            Self::Numbered(n) => f.write_str(&fill(&gettext("Queue {number}"), &[("number", n)])),
        }
    }
}

/// Handle of an operation, shown to the user as its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(u64);

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationState {
    /// In a numbered queue behind another operation, or its queue is paused.
    Waiting,
    Running,
    Paused,
}

/// Snapshot of one operation for display.
#[derive(Debug, Clone)]
pub struct OperationInfo {
    pub id: OperationId,
    pub queue: QueueId,
    pub description: String,
    pub state: OperationState,
    pub progress: Progress,
    /// Running time so far, `None` before it started.
    pub elapsed: Option<Duration>,
    /// Question waiting for an answer.
    pub question: Option<Question>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// Stopped by the user or by an Abort answer.
    Aborted,
}

/// An operation that ended, see [`OperationsManager::take_finished`].
#[derive(Debug, Clone)]
pub struct Finished {
    pub id: OperationId,
    pub description: String,
    pub outcome: Outcome,
}

/// Runs operations in threads and keeps them in queues.
///
/// Cheap to clone, all clones share the same operations.
#[derive(Clone, Default)]
pub struct OperationsManager {
    inner: Arc<Mutex<Inner>>,
}

impl fmt::Debug for OperationsManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OperationsManager").field("operations", &self.operations().len()).finish()
    }
}

#[derive(Default)]
struct Inner {
    last_id: u64,
    /// Empty queues are removed, as in the original.
    queues: BTreeMap<QueueId, Queue>,
    finished: Vec<Finished>,
}

#[derive(Default)]
struct Queue {
    paused: bool,
    items: Vec<Item>,
}

struct Item {
    id: OperationId,
    description: String,
    control: Arc<Control>,
    /// The operation until its thread starts.
    pending: Option<Box<dyn Operation>>,
}

impl OperationsManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an operation to `queue`. In the free queue it starts at once,
    /// in a numbered one when the operations before it are done.
    #[expect(
        clippy::must_use_candidate,
        reason = "the id is for following the operation, a fire-and-forget caller may ignore it"
    )]
    pub fn add(&self, operation: Box<dyn Operation>, queue: QueueId) -> OperationId {
        let mut inner = self.lock();
        inner.last_id += 1;
        let id = OperationId(inner.last_id);
        let item = Item { id, description: operation.description(), control: Arc::default(), pending: Some(operation) };

        let target = inner.queues.entry(queue).or_default();
        target.items.push(item);
        let start_now = match queue {
            QueueId::Free => true,
            QueueId::Numbered(_) => target.items.len() == 1 && !target.paused,
        };
        if start_now && let Some(item) = target.items.last_mut() {
            self.start(item);
        }
        id
    }

    /// The lowest queue number not in use, for "New queue".
    #[must_use]
    pub fn new_queue_id(&self) -> QueueId {
        let inner = self.lock();
        let free_number = (1..=u32::MAX).find(|n| !inner.queues.contains_key(&QueueId::Numbered(*n))).unwrap_or(1);
        QueueId::Numbered(free_number)
    }

    /// All operations, the free queue first, then queues by number, each in
    /// the order they will run.
    #[must_use]
    pub fn operations(&self) -> Vec<OperationInfo> {
        let inner = self.lock();
        inner
            .queues
            .iter()
            .flat_map(|(queue_id, queue)| {
                queue.items.iter().map(move |item| {
                    let control = item.control.lock();
                    let state = if item.pending.is_some() {
                        OperationState::Waiting
                    } else if control.paused {
                        OperationState::Paused
                    } else {
                        OperationState::Running
                    };
                    OperationInfo {
                        id: item.id,
                        queue: *queue_id,
                        description: item.description.clone(),
                        state,
                        progress: control.progress.clone(),
                        elapsed: control.started.map(|started| started.elapsed()),
                        question: control.question.clone(),
                    }
                })
            })
            .collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().queues.is_empty()
    }

    /// Whether Pause applies to `id`: the operation itself in the free
    /// queue, its whole queue otherwise.
    #[must_use]
    pub fn is_paused(&self, id: OperationId) -> bool {
        let inner = self.lock();
        let Some((queue_id, index)) = inner.find(id) else { return false };
        let queue = &inner.queues[&queue_id];
        match queue_id {
            QueueId::Free => queue.items[index].control.lock().paused,
            QueueId::Numbered(_) => queue.paused,
        }
    }

    /// Pause / Start. An operation of the free queue pauses alone, in a
    /// numbered queue the whole queue pauses, so that the next operation
    /// does not start either (`btnPauseStartClick` of the original).
    pub fn set_paused(&self, id: OperationId, paused: bool) {
        let mut inner = self.lock();
        let Some((queue_id, index)) = inner.find(id) else { return };
        let Some(queue) = inner.queues.get_mut(&queue_id) else { return };
        match queue_id {
            QueueId::Free => queue.items[index].control.set_paused(paused),
            QueueId::Numbered(_) => {
                queue.paused = paused;
                let Some(first) = queue.items.first_mut() else { return };
                if first.pending.is_some() {
                    if !paused {
                        self.start(first);
                    }
                } else {
                    first.control.set_paused(paused);
                }
            }
        }
    }

    pub fn toggle_pause(&self, id: OperationId) {
        self.set_paused(id, !self.is_paused(id));
    }

    /// Stops all operations of a queue (Cancel on a numbered queue).
    pub fn stop_queue(&self, queue: QueueId) {
        let ids: Vec<OperationId> = {
            let inner = self.lock();
            let Some(queue) = inner.queues.get(&queue) else { return };
            // Waiting ones first, so that stopping the running one starts nothing.
            queue.items.iter().rev().map(|item| item.id).collect()
        };
        for id in ids {
            self.stop(id);
        }
    }

    /// Stops an operation. One that has not started yet is just removed.
    pub fn stop(&self, id: OperationId) {
        let mut inner = self.lock();
        let Some((queue_id, index)) = inner.find(id) else { return };
        let Some(queue) = inner.queues.get_mut(&queue_id) else { return };
        if queue.items[index].pending.is_some() {
            let item = queue.items.remove(index);
            if queue.items.is_empty() {
                inner.queues.remove(&queue_id);
            }
            inner.finished.push(Finished { id, description: item.description, outcome: Outcome::Aborted });
        } else {
            queue.items[index].control.stop();
        }
    }

    /// Answers the pending question of an operation.
    pub fn answer(&self, id: OperationId, answer: Answer) {
        self.answer_with_text(id, answer, None);
    }

    /// Answers a pending question that asks for a text ([`Question::input`]).
    pub fn answer_with_text(&self, id: OperationId, answer: Answer, text: Option<String>) {
        let inner = self.lock();
        if let Some((queue_id, index)) = inner.find(id) {
            inner.queues[&queue_id].items[index].control.answer(answer, text);
        }
    }

    /// Operations that ended since the last call, oldest first.
    #[must_use]
    pub fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut self.lock().finished)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts the thread of `item`. Called with the lock held, the thread
    /// takes it only when it finishes.
    fn start(&self, item: &mut Item) {
        let Some(mut operation) = item.pending.take() else { return };
        let (id, control, manager) = (item.id, Arc::clone(&item.control), self.clone());
        control.lock().started = Some(Instant::now());
        thread::spawn(move || {
            let ctx = OperationContext::new(control);
            let outcome = match operation.run(&ctx) {
                Ok(()) => Outcome::Completed,
                Err(_) => Outcome::Aborted,
            };
            manager.finish(id, outcome);
        });
    }

    /// Removes a finished operation and starts the next one of its queue.
    fn finish(&self, id: OperationId, outcome: Outcome) {
        let mut inner = self.lock();
        let Some((queue_id, index)) = inner.find(id) else { return };
        let Some(queue) = inner.queues.get_mut(&queue_id) else { return };
        let item = queue.items.remove(index);

        if queue.items.is_empty() {
            inner.queues.remove(&queue_id);
        } else if queue_id != QueueId::Free && !queue.paused {
            let next = &mut queue.items[0];
            self.start(next);
        }
        inner.finished.push(Finished { id, description: item.description, outcome });
    }
}

impl Inner {
    fn find(&self, id: OperationId) -> Option<(QueueId, usize)> {
        self.queues
            .iter()
            .find_map(|(queue_id, queue)| queue.items.iter().position(|item| item.id == id).map(|index| (*queue_id, index)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, Receiver, Sender};

    use super::*;
    use crate::operation::Aborted;

    /// Waits for a message from the test before it finishes, so the test
    /// decides when each operation ends. Reports its start through `started`.
    struct Gate {
        name: &'static str,
        release: Receiver<()>,
        started: Sender<&'static str>,
    }

    impl Operation for Gate {
        fn description(&self) -> String {
            self.name.to_owned()
        }

        fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
            self.started.send(self.name).unwrap();
            loop {
                ctx.checkpoint()?;
                if self.release.recv_timeout(Duration::from_millis(5)).is_ok() {
                    return Ok(());
                }
            }
        }
    }

    fn gate(name: &'static str, started: &Sender<&'static str>) -> (Box<dyn Operation>, Sender<()>) {
        let (release_tx, release) = mpsc::channel();
        (Box::new(Gate { name, release, started: started.clone() }), release_tx)
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn states(manager: &OperationsManager) -> Vec<(String, OperationState)> {
        manager.operations().into_iter().map(|info| (info.description, info.state)).collect()
    }

    #[test]
    fn free_queue_runs_side_by_side() {
        let manager = OperationsManager::new();
        let (started_tx, started) = mpsc::channel();
        let (a, release_a) = gate("a", &started_tx);
        let (b, release_b) = gate("b", &started_tx);
        manager.add(a, QueueId::Free);
        manager.add(b, QueueId::Free);

        let mut both = vec![started.recv().unwrap(), started.recv().unwrap()];
        both.sort_unstable();
        assert_eq!(both, ["a", "b"]);

        release_a.send(()).unwrap();
        release_b.send(()).unwrap();
        wait_until(|| manager.is_empty());
        assert_eq!(manager.take_finished().len(), 2);
        assert!(manager.take_finished().is_empty());
    }

    #[test]
    fn numbered_queue_runs_one_after_another() {
        let manager = OperationsManager::new();
        let (started_tx, started) = mpsc::channel();
        let (a, release_a) = gate("a", &started_tx);
        let (b, release_b) = gate("b", &started_tx);
        let queue = QueueId::Numbered(1);
        manager.add(a, queue);
        manager.add(b, queue);

        assert_eq!(started.recv().unwrap(), "a");
        assert_eq!(states(&manager), [("a".into(), OperationState::Running), ("b".into(), OperationState::Waiting)]);
        assert!(started.recv_timeout(Duration::from_millis(30)).is_err(), "b waits for a");

        release_a.send(()).unwrap();
        assert_eq!(started.recv().unwrap(), "b");
        release_b.send(()).unwrap();
        wait_until(|| manager.is_empty());
        let finished: Vec<_> = manager.take_finished().into_iter().map(|f| (f.description, f.outcome)).collect();
        assert_eq!(finished, [("a".into(), Outcome::Completed), ("b".into(), Outcome::Completed)]);
    }

    #[test]
    fn paused_queue_does_not_start_the_next_operation() {
        let manager = OperationsManager::new();
        let (started_tx, started) = mpsc::channel();
        let (a, release_a) = gate("a", &started_tx);
        let (b, release_b) = gate("b", &started_tx);
        let queue = QueueId::Numbered(2);
        manager.add(a, queue);
        // Controlled through b, which stays in the queue the whole time.
        let b_id = manager.add(b, queue);
        assert_eq!(started.recv().unwrap(), "a");

        manager.toggle_pause(b_id);
        assert_eq!(states(&manager)[0].1, OperationState::Paused);
        // Whether a ends now or stays paused, b must not start while the queue is paused.
        release_a.send(()).unwrap();
        assert!(started.recv_timeout(Duration::from_millis(50)).is_err());

        manager.toggle_pause(b_id);
        assert_eq!(started.recv_timeout(Duration::from_secs(5)).unwrap(), "b");
        release_b.send(()).unwrap();
        wait_until(|| manager.is_empty());
    }

    #[test]
    fn stop_aborts_running_and_removes_waiting_operations() {
        let manager = OperationsManager::new();
        let (started_tx, started) = mpsc::channel();
        let (a, _release_a) = gate("a", &started_tx);
        let (b, _release_b) = gate("b", &started_tx);
        let queue = QueueId::Numbered(1);
        let a_id = manager.add(a, queue);
        let b_id = manager.add(b, queue);
        assert_eq!(started.recv().unwrap(), "a");

        manager.stop(b_id);
        assert_eq!(states(&manager).len(), 1);
        manager.stop(a_id);
        wait_until(|| manager.is_empty());
        assert!(started.recv_timeout(Duration::from_millis(30)).is_err(), "stopped b must never start");
        let outcomes: Vec<_> = manager.take_finished().into_iter().map(|f| f.outcome).collect();
        assert_eq!(outcomes, [Outcome::Aborted, Outcome::Aborted]);
    }

    #[test]
    fn stopping_a_queue_stops_all_and_starts_nothing() {
        let manager = OperationsManager::new();
        let (started_tx, started) = mpsc::channel();
        let queue = QueueId::Numbered(3);
        let mut releases = Vec::new();
        for name in ["a", "b", "c"] {
            let (op, release) = gate(name, &started_tx);
            releases.push(release);
            let _ = manager.add(op, queue);
        }
        assert_eq!(started.recv().unwrap(), "a");
        let a_id = manager.operations()[0].id;
        assert!(!manager.is_paused(a_id));
        manager.set_paused(a_id, true);
        assert!(manager.is_paused(a_id));

        manager.stop_queue(queue);
        wait_until(|| manager.is_empty());
        assert!(started.recv_timeout(Duration::from_millis(30)).is_err());
        assert_eq!(manager.take_finished().len(), 3);
    }

    #[test]
    fn new_queue_takes_the_lowest_unused_number() {
        let manager = OperationsManager::new();
        assert_eq!(manager.new_queue_id(), QueueId::Numbered(1));
        let (started_tx, _started) = mpsc::channel();
        let (a, _release) = gate("a", &started_tx);
        manager.add(a, QueueId::Numbered(1));
        assert_eq!(manager.new_queue_id(), QueueId::Numbered(2));
        let id = manager.operations()[0].id;
        manager.stop(id);
        wait_until(|| manager.is_empty());
    }

    #[test]
    fn questions_are_visible_and_answered_through_the_manager() {
        struct Asking;
        impl Operation for Asking {
            fn description(&self) -> String {
                "asking".into()
            }
            fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted> {
                let question = Question::new("go on?".into(), vec![Answer::Yes, Answer::Abort], Answer::Yes, Answer::Abort);
                match ctx.ask(question)? {
                    Answer::Yes => Ok(()),
                    _ => Err(Aborted),
                }
            }
        }

        let manager = OperationsManager::new();
        let id = manager.add(Box::new(Asking), QueueId::Free);
        wait_until(|| manager.operations().first().is_some_and(|info| info.question.is_some()));
        assert_eq!(manager.operations()[0].question.as_ref().unwrap().text, "go on?");

        manager.answer(id, Answer::Yes);
        wait_until(|| manager.is_empty());
        assert_eq!(manager.take_finished()[0].outcome, Outcome::Completed);
    }
}
