//! Monotonic revisions and protocol-neutral write preconditions.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::DomainError;

/// Non-negative monotonic revision stored by SQLite as a signed integer.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct Revision(u64);

impl Revision {
    /// Initial revision for a file or settings record.
    pub const ZERO: Self = Self(0);

    /// Construct a revision from a non-negative number.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the numeric revision.
    pub const fn value(self) -> u64 {
        self.0
    }

    /// Convert to SQLite's signed integer representation.
    pub fn as_i64(self) -> Result<i64, DomainError> {
        i64::try_from(self.0).map_err(|_| DomainError::RevisionOverflow)
    }

    /// Return the next revision or a typed overflow error.
    pub fn next(self) -> Result<Self, DomainError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DomainError::RevisionOverflow)
    }
}

impl TryFrom<i64> for Revision {
    type Error = DomainError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map(Self)
            .map_err(|_| DomainError::NegativeRevision)
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Expected state for a canonical write.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WritePrecondition {
    /// Do not require an existing revision.
    Unconditional,
    /// The target must not currently exist.
    CreateOnly,
    /// The target must exist at this exact revision.
    ExactRevision(Revision),
}

impl WritePrecondition {
    /// Check the precondition against the current optional file revision.
    pub fn check(self, current: Option<Revision>) -> Result<(), DomainError> {
        match self {
            Self::Unconditional => Ok(()),
            Self::CreateOnly if current.is_none() => Ok(()),
            Self::CreateOnly => Err(DomainError::PreconditionFailed {
                reason: "entry already exists",
            }),
            Self::ExactRevision(expected) => match current {
                Some(actual) if actual == expected => Ok(()),
                Some(actual) => Err(DomainError::RevisionConflict {
                    expected,
                    current: actual,
                }),
                None => Err(DomainError::PreconditionFailed {
                    reason: "entry does not exist",
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Revision, WritePrecondition};
    use crate::DomainError;

    #[test]
    fn revisions_are_checked_and_sqlite_compatible() {
        assert_eq!(Revision::ZERO.next().unwrap(), Revision::new(1));
        assert_eq!(Revision::new(42).as_i64().unwrap(), 42);
        assert_eq!(
            Revision::try_from(-1).unwrap_err(),
            DomainError::NegativeRevision
        );
        assert_eq!(
            Revision::new(u64::MAX).next().unwrap_err(),
            DomainError::RevisionOverflow
        );
    }

    #[test]
    fn write_preconditions_distinguish_create_and_revision_conflicts() {
        assert!(WritePrecondition::CreateOnly.check(None).is_ok());
        assert!(matches!(
            WritePrecondition::CreateOnly.check(Some(Revision::new(1))),
            Err(DomainError::PreconditionFailed { .. })
        ));
        assert!(
            WritePrecondition::ExactRevision(Revision::new(2))
                .check(Some(Revision::new(2)))
                .is_ok()
        );
        assert!(matches!(
            WritePrecondition::ExactRevision(Revision::new(2)).check(Some(Revision::new(3))),
            Err(DomainError::RevisionConflict { .. })
        ));
    }
}
