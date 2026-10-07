//! Identifiers of runs, bus events and deliveries. Each is its own type so
//! one can't be passed where another is meant; all three are plain numbers
//! in the database and on the wire.

use duckdb::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            /// An id as given on the command line, over RPC or in the
            /// database; it isn't checked to exist.
            pub const fn new(n: i64) -> $name {
                $name(n)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> duckdb::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<$name> {
                i64::column_result(value).map($name)
            }
        }
    };
}

id!(
    /// A run's id.
    RunId
);
id!(
    /// A published bus event's id.
    EventId
);
id!(
    /// The id of one subscriber's delivery of a bus event.
    DeliveryId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_numbers_outside() {
        let id = RunId::new(42);
        assert_eq!(id.to_string(), "42");
        assert_eq!(serde_json::json!(id), serde_json::json!(42));
        assert_eq!(
            serde_json::from_value::<EventId>(serde_json::json!(7)).unwrap(),
            EventId::new(7)
        );
        assert_eq!(DeliveryId::new(3), DeliveryId::new(3));
        assert!(RunId::new(2) > RunId::new(1));
    }
}
