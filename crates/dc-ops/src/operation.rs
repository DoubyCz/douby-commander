use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use dc_i18n::pgettext;
use std::time::Instant;

/// How far an operation got. Read by the GUI while the operation runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Item being processed right now.
    pub current: Option<PathBuf>,
    /// Where the current item goes (copy, move); `None` for operations
    /// without a target.
    pub target: Option<PathBuf>,
    pub done_files: u64,
    /// `None` while the operation is still counting what to do.
    pub total_files: Option<u64>,
    pub done_bytes: u64,
    pub total_bytes: Option<u64>,
    /// Bytes of the current file done and in all, for operations that go
    /// through file contents.
    pub file_done_bytes: u64,
    pub file_total_bytes: Option<u64>,
    /// The done part is measured in bytes (copying), not in files (deleting).
    pub by_bytes: bool,
}

impl Progress {
    /// Done part between 0 and 1, `None` while counting. In bytes when the
    /// operation measures so and there are any, else in files.
    #[must_use]
    pub fn fraction(&self) -> Option<f64> {
        let files = self.total_files?;
        match self.total_bytes {
            Some(bytes) if self.by_bytes && bytes > 0 => Some(ratio(self.done_bytes, bytes)),
            _ => Some(ratio(self.done_files, files)),
        }
    }

    /// Done part of the current file between 0 and 1.
    #[must_use]
    pub fn file_fraction(&self) -> Option<f64> {
        self.file_total_bytes.map(|total| ratio(self.file_done_bytes, total))
    }
}

fn ratio(done: u64, total: u64) -> f64 {
    if total == 0 {
        return 1.0;
    }
    #[expect(clippy::cast_precision_loss, reason = "precision loss only matters above 2^52")]
    let ratio = (done.min(total) as f64) / (total as f64);
    ratio
}

/// Possible answers to a [`Question`], as the buttons of the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Answer {
    Yes,
    No,
    All,
    Skip,
    SkipAll,
    Retry,
    Abort,
    Cancel,
    IgnoreAll,
    Overwrite,
    OverwriteAll,
    OverwriteOlder,
    OverwriteSmaller,
    OverwriteLarger,
    Rename,
    AutoRenameSource,
    AutoRenameTarget,
    Merge,
    MergeAll,
}

impl Answer {
    /// Translated button caption, `_` marks the mnemonic (`rsDlgButton*` of the original).
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Yes => pgettext("button", "_Yes"),
            Self::No => pgettext("button", "_No"),
            Self::All => pgettext("button", "A_ll"),
            Self::Skip => pgettext("button", "_Skip"),
            Self::SkipAll => pgettext("button", "S_kip All"),
            Self::Retry => pgettext("button", "_Retry"),
            Self::Abort => pgettext("button", "_Abort"),
            Self::Cancel => pgettext("button", "_Cancel"),
            Self::IgnoreAll => pgettext("button", "I_gnore All"),
            Self::Overwrite => pgettext("button", "_Overwrite"),
            Self::OverwriteAll => pgettext("button", "Overwrite _All"),
            Self::OverwriteOlder => pgettext("button", "Overwrite All Ol_der"),
            Self::OverwriteSmaller => pgettext("button", "Overwrite All S_maller"),
            Self::OverwriteLarger => pgettext("button", "Overwrite All _Larger"),
            Self::Rename => pgettext("button", "R_ename"),
            Self::AutoRenameSource => pgettext("button", "A_uto-rename source files"),
            Self::AutoRenameTarget => pgettext("button", "Auto-rename tar_get files"),
            Self::Merge => pgettext("button", "_Merge"),
            Self::MergeAll => pgettext("button", "Mer_ge All"),
        }
    }
}

/// Something only the user can decide, e.g. what to do with a file that
/// cannot be deleted. The operation waits until it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// What happened or what is asked, translated.
    pub text: String,
    /// Further detail, e.g. the reason of a failure; may be empty.
    pub detail: String,
    /// In the order the buttons are shown.
    pub answers: Vec<Answer>,
    /// Answer of Enter.
    pub default: Answer,
    /// Answer when the question is dismissed (Esc, closing the dialog).
    pub cancel: Answer,
    /// The question asks for a text (a new name), prefilled with this.
    /// Answered with the text and [`Answer::Yes`], or with the cancel answer.
    pub input: Option<String>,
}

impl Question {
    /// A question without detail and without a text to enter.
    #[must_use]
    pub fn new(text: String, answers: Vec<Answer>, default: Answer, cancel: Answer) -> Self {
        Self { text, detail: String::new(), answers, default, cancel, input: None }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = detail;
        self
    }
}

/// The operation was stopped by the user or one of its questions was
/// answered with Abort. Whatever was done before stays done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aborted;

/// A file operation, run in a thread of its own by the [`OperationsManager`].
///
/// [`OperationsManager`]: crate::OperationsManager
pub trait Operation: Send {
    /// What the operation does, for window titles and lists ("Deleting").
    fn description(&self) -> String;

    /// Does the work. Call [`OperationContext::checkpoint`] between items so
    /// that pausing and stopping work, report progress through the context
    /// and ask the user through it.
    ///
    /// # Errors
    /// [`Aborted`] when stopped or aborted by an answer.
    fn run(&mut self, ctx: &OperationContext) -> Result<(), Aborted>;
}

/// State shared between an operation thread and whoever controls it.
#[derive(Debug, Default)]
pub(crate) struct Control {
    state: Mutex<ControlState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
pub(crate) struct ControlState {
    pub progress: Progress,
    pub paused: bool,
    pub stop: bool,
    pub question: Option<Question>,
    pub answer: Option<Answer>,
    /// Text entered with the answer of a question with `input`.
    pub answer_text: Option<String>,
    pub started: Option<Instant>,
}

impl Control {
    /// A panic in an operation thread must not take the GUI down with it,
    /// the state stays usable after a poisoned lock.
    pub(crate) fn lock(&self) -> MutexGuard<'_, ControlState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.lock().paused = paused;
        self.changed.notify_all();
    }

    pub(crate) fn stop(&self) {
        self.lock().stop = true;
        self.changed.notify_all();
    }

    pub(crate) fn answer(&self, answer: Answer, text: Option<String>) {
        let mut state = self.lock();
        if state.question.take().is_some() {
            state.answer = Some(answer);
            state.answer_text = text;
        }
        drop(state);
        self.changed.notify_all();
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, ControlState>) -> MutexGuard<'a, ControlState> {
        self.changed.wait(guard).unwrap_or_else(PoisonError::into_inner)
    }
}

type Answerer = Box<dyn Fn(&Question) -> (Answer, Option<String>) + Send + Sync>;

/// What a running operation uses to talk to the outside world.
pub struct OperationContext {
    control: Arc<Control>,
    /// Answers questions directly instead of waiting for the user.
    answerer: Option<Answerer>,
}

impl std::fmt::Debug for OperationContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OperationContext").field("control", &self.control).finish_non_exhaustive()
    }
}

impl OperationContext {
    pub(crate) fn new(control: Arc<Control>) -> Self {
        Self { control, answerer: None }
    }

    /// Runs an operation without a manager (tests, scripts): questions are
    /// answered by `answerer` right away.
    pub fn with_answerer(answerer: impl Fn(&Question) -> Answer + Send + Sync + 'static) -> Self {
        Self::with_replies(move |question| (answerer(question), None))
    }

    /// As [`Self::with_answerer`], with the text for questions that ask for one.
    pub fn with_replies(replies: impl Fn(&Question) -> (Answer, Option<String>) + Send + Sync + 'static) -> Self {
        Self { control: Arc::default(), answerer: Some(Box::new(replies)) }
    }

    /// Waits while the operation is paused.
    ///
    /// # Errors
    /// [`Aborted`] once the operation is stopped.
    pub fn checkpoint(&self) -> Result<(), Aborted> {
        let mut state = self.control.lock();
        while state.paused && !state.stop {
            state = self.control.wait(state);
        }
        if state.stop { Err(Aborted) } else { Ok(()) }
    }

    /// Asks the user and waits for the answer.
    ///
    /// # Errors
    /// [`Aborted`] when the operation is stopped while waiting.
    pub fn ask(&self, question: Question) -> Result<Answer, Aborted> {
        self.ask_with_text(question).map(|(answer, _)| answer)
    }

    /// Asks for a text, e.g. a new name (`rsEditNewFileName`), prefilled
    /// with `initial`. `None` when the user cancels.
    ///
    /// # Errors
    /// [`Aborted`] when the operation is stopped while waiting.
    pub fn ask_text(&self, text: String, initial: String) -> Result<Option<String>, Aborted> {
        let question = Question { input: Some(initial), ..Question::new(text, vec![Answer::Cancel, Answer::Yes], Answer::Yes, Answer::Cancel) };
        let (answer, text) = self.ask_with_text(question)?;
        Ok(text.filter(|_| answer == Answer::Yes))
    }

    fn ask_with_text(&self, question: Question) -> Result<(Answer, Option<String>), Aborted> {
        if let Some(answerer) = &self.answerer {
            return Ok(answerer(&question));
        }
        let mut state = self.control.lock();
        state.answer = None;
        state.answer_text = None;
        state.question = Some(question);
        drop(state);
        self.control.changed.notify_all();

        let mut state = self.control.lock();
        loop {
            if state.stop {
                state.question = None;
                return Err(Aborted);
            }
            if let Some(answer) = state.answer.take() {
                return Ok((answer, state.answer_text.take()));
            }
            state = self.control.wait(state);
        }
    }

    pub fn update_progress(&self, update: impl FnOnce(&mut Progress)) {
        update(&mut self.control.lock().progress);
    }

    #[must_use]
    pub fn progress(&self) -> Progress {
        self.control.lock().progress.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;

    fn question() -> Question {
        Question::new("?".into(), vec![Answer::Retry, Answer::Abort], Answer::Retry, Answer::Abort)
    }

    #[test]
    fn ask_waits_for_the_answer_from_another_thread() {
        let control = Arc::new(Control::default());
        let ctx = OperationContext::new(Arc::clone(&control));
        let worker = thread::spawn(move || ctx.ask(question()));

        while control.lock().question.is_none() {
            thread::sleep(Duration::from_millis(1));
        }
        control.answer(Answer::Retry, None);
        assert_eq!(worker.join().unwrap(), Ok(Answer::Retry));
        assert!(control.lock().question.is_none());
    }

    #[test]
    fn stop_releases_a_waiting_question_and_a_pause() {
        let control = Arc::new(Control::default());
        let ctx = OperationContext::new(Arc::clone(&control));
        let asking = thread::spawn(move || ctx.ask(question()));
        while control.lock().question.is_none() {
            thread::sleep(Duration::from_millis(1));
        }
        control.stop();
        assert_eq!(asking.join().unwrap(), Err(Aborted));

        let control = Arc::new(Control::default());
        control.set_paused(true);
        let ctx = OperationContext::new(Arc::clone(&control));
        let paused = thread::spawn(move || ctx.checkpoint());
        thread::sleep(Duration::from_millis(20));
        assert!(!paused.is_finished(), "checkpoint must wait while paused");
        control.stop();
        assert_eq!(paused.join().unwrap(), Err(Aborted));
    }

    #[test]
    fn fraction_by_files() {
        let mut progress = Progress::default();
        assert_eq!(progress.fraction(), None);
        progress.total_files = Some(4);
        progress.done_files = 1;
        assert_eq!(progress.fraction(), Some(0.25));
        progress.total_files = Some(0);
        assert_eq!(progress.fraction(), Some(1.0));
    }
}
