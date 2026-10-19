//! File operations running in background threads.
//!
//! Follows the model of the original (`uoperationsmanager.pas`,
//! `TFileSourceOperation`): every operation runs in its own thread, reports
//! progress, can be paused and stopped, and asks the user through questions
//! it waits for (Skip, Retry, ...). The [`OperationsManager`] keeps the
//! operations in queues: the free queue runs everything at once, numbered
//! queues run their operations one after another.
//!
//! Nothing here knows about the GUI. The GUI polls
//! [`OperationsManager::operations`] for progress and pending questions and
//! answers them with [`OperationsManager::answer`].

mod delete;
mod manager;
mod operation;
mod target;
mod transfer;

pub use delete::{DeleteMode, DeleteOperation};
pub use target::{Destination, apply_mask};
pub use transfer::{DirectoryExists, FileExists, FollowLinks, PropertyError, TransferMode, TransferOperation, TransferOptions};
pub use manager::{Finished, OperationId, OperationInfo, OperationState, OperationsManager, Outcome, QueueId};
pub use operation::{Aborted, Answer, Operation, OperationContext, Progress, Question};
