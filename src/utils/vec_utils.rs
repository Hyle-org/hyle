use anyhow::{Error, Result};

pub trait SequenceOption<T> {
    fn sequence(self) -> Option<Vec<T>>;
}

/// Helper to transform a `Vec<Option<T>>` to an `Option<Vec<T>>`
/// if a single value of the Vec is None, then the result is None
impl<T> SequenceOption<T> for Vec<Option<T>> {
    fn sequence(self) -> Option<Vec<T>> {
        self.into_iter().collect::<Option<Vec<T>>>()
    }
}

pub trait SequenceResult<T> {
    fn sequence(self) -> Result<Vec<T>, Error>;
}

/// Helper to transform a `Vec<Option<T>>` to an `Option<Vec<T>>`
/// if a single value of the Vec is Error, then the result is Error
impl<T> SequenceResult<T> for Vec<Result<T, Error>> {
    fn sequence(self) -> Result<Vec<T>, Error> {
        self.into_iter().collect::<Result<Vec<T>, Error>>()
    }
}
