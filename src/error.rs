use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub enum GitError {
    NoChanges,
    NoCommitMessage,
    CommandFailed(String),
    NoToken,
    NoInternet,
    Other(String),
}

impl Error for GitError {}

pub type Result<T = ()> = std::result::Result<T, GitError>;

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::NoChanges => write!(f, "No changes to commit"),
            GitError::NoCommitMessage => write!(f, "No commit message provided"),
            GitError::CommandFailed(msg) => write!(f, "Command failed: {}", msg),
            GitError::NoToken => write!(f, "No GitHub token found"),
            GitError::NoInternet => write!(f, "No internet connection"),
            GitError::Other(msg) => write!(f, "{}", msg),
        }
    }
}
