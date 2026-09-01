// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod blob;
mod datetime;
mod numeric;

pub use self::blob::Blob;
pub use self::datetime::SqlDateTime;
pub use self::numeric::NumericU64;

use crate::db::{LabelledPoolApi, Pool};
use golem_common::{SafeDisplay, error_forwarding};
use sqlx::error::ErrorKind;
use sqlx::{Database, Encode, Type};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("Unique violation repository error: {0}")]
    UniqueViolation(String),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl RepoError {
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, RepoError::UniqueViolation(_))
    }

    pub fn is_transient(&self) -> bool {
        match self {
            RepoError::InternalError(err) => err
                .downcast_ref::<sqlx::Error>()
                .is_some_and(is_transient_sqlx_error),
            RepoError::UniqueViolation(_) => false,
        }
    }

    /// Returns `true` if this error is a connection pool acquisition timeout.
    ///
    /// Unlike [`Self::is_transient`], this excludes mid-query I/O errors: a pool timeout happens
    /// before any statement runs, so retrying the operation cannot observe a partially applied
    /// write. This makes it safe to retry even non-idempotent operations.
    pub fn is_pool_timeout(&self) -> bool {
        match self {
            RepoError::InternalError(err) => err
                .downcast_ref::<sqlx::Error>()
                .is_some_and(|sqlx_err| matches!(sqlx_err, sqlx::Error::PoolTimedOut)),
            RepoError::UniqueViolation(_) => false,
        }
    }
}

/// Whether a failed query is worth retrying.
///
/// Exposed separately from [`RepoError::is_transient`] so that a caller holding a `sqlx::Error`
/// that never became a `RepoError` - a pool or migration failure during storage initialization -
/// classifies it the same way.
pub fn is_transient_sqlx_error(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) => true,
        sqlx::Error::Database(db_err) => is_transient_database_error(db_err.as_ref()),
        _ => false,
    }
}

/// Whether a backend's own error response describes a condition worth retrying.
///
/// Classifying only on `sqlx::Error::Io` misses the failure a managed-database switchover actually
/// produces: PostgreSQL reports the old writer going away as a *server response* with a SQLSTATE,
/// not as a transport error, so the connection closes only after the FATAL has been delivered. An
/// error reaching us as `Error::Database` is therefore not evidence that retrying is pointless.
fn is_transient_database_error(err: &dyn sqlx::error::DatabaseError) -> bool {
    let Some(code) = err.code() else {
        return false;
    };
    let code: &str = code.as_ref();

    // A SQLSTATE is always exactly five characters; SQLite reports a bare result code instead.
    // Length is the discriminator rather than "does it parse as a number", because plenty of
    // SQLSTATEs are all digits - `08000` would otherwise be read as the SQLite code 8000.
    if code.len() != 5 {
        // SQLite, sometimes an extended code whose low byte is the primary one. `BUSY` and
        // `LOCKED` are both lock contention that clears on its own.
        return code
            .parse::<u32>()
            .is_ok_and(|numeric| matches!(numeric & 0xFF, 5 | 6));
    }

    // PostgreSQL SQLSTATE. Class 08 is the whole connection-exception family.
    if code.starts_with("08") {
        return true;
    }

    matches!(
        code,
        // Class 57, operator intervention: the server is shutting down, has just crashed, or is
        // not yet accepting connections. All three are what a failover looks like from the client.
        "57P01" | "57P02" | "57P03"
        // Class 40, transaction rollback. The statement was rolled back rather than half-applied,
        // so a retry starts from the same state the first attempt did.
        | "40001" | "40P01"
    )
}

error_forwarding!(RepoError);

impl From<sqlx::Error> for RepoError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db_error) = error.as_database_error()
            && db_error.kind() == ErrorKind::UniqueViolation
        {
            RepoError::UniqueViolation(db_error.to_string())
        } else {
            RepoError::InternalError(error.into())
        }
    }
}

impl SafeDisplay for RepoError {
    fn to_safe_string(&self) -> String {
        match self {
            RepoError::InternalError(_) => "Internal repository error".to_string(),
            RepoError::UniqueViolation(_) => {
                "Internal repository error (unique key violation)".to_string()
            }
        }
    }
}

pub type RepoResult<T> = Result<T, RepoError>;

pub type PoolLabelledTransaction<T> =
    <<T as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction;

pub trait ResultExt<T> {
    fn none_on_unique_violation(self) -> RepoResult<Option<T>>;

    fn false_on_unique_violation(self) -> RepoResult<bool>;

    fn to_error_on_unique_violation<E: From<RepoError>>(self, business_error: E) -> Result<T, E>;
}

impl<T> ResultExt<T> for RepoResult<T> {
    fn none_on_unique_violation(self) -> RepoResult<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) if err.is_unique_violation() => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn false_on_unique_violation(self) -> RepoResult<bool> {
        match self {
            Ok(_) => Ok(true),
            Err(err) if err.is_unique_violation() => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn to_error_on_unique_violation<E: From<RepoError>>(self, business_error: E) -> Result<T, E> {
        match self {
            Ok(value) => Ok(value),
            Err(err) if err.is_unique_violation() => Err(business_error),
            Err(err) => Err(err.into()),
        }
    }
}

type BindFn<'q, DB, R> = Box<
    dyn FnOnce(
            sqlx::query::QueryAs<'q, DB, R, <DB as Database>::Arguments<'q>>,
        ) -> sqlx::query::QueryAs<'q, DB, R, <DB as Database>::Arguments<'q>>
        + 'q
        + Send,
>;

pub struct BindingsStack<'q, DB: Database, R> {
    next: usize,
    bind_fns: Vec<BindFn<'q, DB, R>>,
}

impl<'q, DB: Database, R> BindingsStack<'q, DB, R> {
    pub fn new(start: usize) -> Self {
        Self {
            next: start,
            bind_fns: Vec::new(),
        }
    }

    pub fn push<'bind: 'q, T: 'bind + Encode<'q, DB> + Type<DB> + Send>(
        &mut self,
        value: T,
    ) -> usize {
        let idx = self.next;
        self.next += 1;

        self.bind_fns.push(Box::new(move |q| q.bind(value)));

        idx
    }

    pub fn apply(
        self,
        query: sqlx::query::QueryAs<'q, DB, R, <DB as Database>::Arguments<'q>>,
    ) -> sqlx::query::QueryAs<'q, DB, R, <DB as Database>::Arguments<'q>> {
        let mut result = query;
        for bind_fn in self.bind_fns {
            result = bind_fn(result);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::RepoError;
    use sqlx::error::{DatabaseError, ErrorKind};
    use std::borrow::Cow;
    use std::error::Error as StdError;
    use test_r::test;

    /// A backend error response carrying a chosen code, which is the only thing the classification
    /// looks at. Real ones cannot be constructed outside their driver.
    #[derive(Debug)]
    struct FakeDatabaseError(&'static str);

    impl std::fmt::Display for FakeDatabaseError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "database error {}", self.0)
        }
    }

    impl StdError for FakeDatabaseError {}

    impl DatabaseError for FakeDatabaseError {
        fn message(&self) -> &str {
            "database error"
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn StdError + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn StdError + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn StdError + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    fn database_error(code: &'static str) -> RepoError {
        RepoError::from(sqlx::Error::Database(Box::new(FakeDatabaseError(code))))
    }

    /// The failure a managed-database switchover actually produces. PostgreSQL answers with a
    /// FATAL and *then* closes the connection, so it arrives as a server response rather than as
    /// an I/O error - classifying only on the transport would let the retry budget go unused for
    /// the one event it was sized to cover.
    #[test]
    fn a_server_reported_shutdown_is_transient() {
        for code in ["57P01", "57P02", "57P03"] {
            assert!(
                database_error(code).is_transient(),
                "{code} (operator intervention) should be retried"
            );
        }
    }

    #[test]
    fn the_connection_exception_class_is_transient() {
        for code in ["08000", "08001", "08003", "08004", "08006"] {
            assert!(
                database_error(code).is_transient(),
                "{code} (connection exception) should be retried"
            );
        }
    }

    /// Rolled back rather than half-applied, so a retry starts from the state the first attempt
    /// saw.
    #[test]
    fn serialization_failure_and_deadlock_are_transient() {
        assert!(database_error("40001").is_transient());
        assert!(database_error("40P01").is_transient());
    }

    /// SQLite reports a number, sometimes an extended one whose low byte is the primary code.
    #[test]
    fn sqlite_lock_contention_is_transient() {
        for code in ["5", "6", "261", "517"] {
            assert!(
                database_error(code).is_transient(),
                "SQLite {code} (BUSY/LOCKED family) should be retried"
            );
        }
    }

    /// The point of classifying at all: a rejected statement is not a blip, and retrying it just
    /// spends the budget to fail the same way.
    #[test]
    fn a_rejected_statement_is_not_transient() {
        for code in ["23505", "42601", "42P01", "22001"] {
            assert!(
                !database_error(code).is_transient(),
                "{code} should not be retried"
            );
        }
        // SQLite SQLITE_CONSTRAINT, and its extended NOTNULL variant.
        assert!(!database_error("19").is_transient());
        assert!(!database_error("1299").is_transient());
    }

    /// Widening `is_transient` must not widen what may be retried when the operation is not
    /// idempotent: only a pool timeout carries the "never reached the backend" guarantee.
    #[test]
    fn a_server_reported_error_is_never_a_pool_timeout() {
        assert!(!database_error("57P01").is_pool_timeout());
        assert!(!database_error("08006").is_pool_timeout());
    }

    #[test]
    fn pool_timeout_is_transient_and_a_pool_timeout() {
        let error = RepoError::from(sqlx::Error::PoolTimedOut);
        assert!(error.is_transient());
        assert!(error.is_pool_timeout());
    }

    /// The distinction callers depend on: an I/O error may have reached the database, a pool
    /// acquisition timeout cannot have.
    #[test]
    fn mid_query_io_error_is_transient_but_not_a_pool_timeout() {
        let error = RepoError::from(sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        )));
        assert!(error.is_transient());
        assert!(!error.is_pool_timeout());
    }

    #[test]
    fn other_errors_are_neither() {
        let error = RepoError::from(sqlx::Error::RowNotFound);
        assert!(!error.is_transient());
        assert!(!error.is_pool_timeout());

        let error = RepoError::UniqueViolation("duplicate".to_string());
        assert!(!error.is_transient());
        assert!(!error.is_pool_timeout());
    }
}
