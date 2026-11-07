//! `FromRow` for tuples, read by position: `(i64, String)` takes the first two columns.
//!
//! One `macro_rules!` macro writes the impls; each line of the invocation is one arity, with
//! each element's type parameter next to its position.

use tokio_postgres::{Column, Row};

use crate::error::Error;
use crate::row::{FromColumn, FromRow, column_at, get};

macro_rules! tuple_impls {
    ($( ($($T:ident $i:tt),+) )+) => {
        $(
            impl<$($T: FromColumn),+> FromRow for ($($T,)+) {
                type Columns = ();

                fn resolve(columns: &[Column]) -> Result<(), Error> {
                    $( column_at::<$T>(columns, "tuple", stringify!($i), $i)?; )+
                    Ok(())
                }

                fn from_row_at(row: &Row, _: &()) -> Result<Self, Error> {
                    Ok(($( get::<$T>(row, $i, "tuple", stringify!($i))?, )+))
                }
            }
        )+
    };
}

tuple_impls! {
    (A 0)
    (A 0, B 1)
    (A 0, B 1, C 2)
    (A 0, B 1, C 2, D 3)
    (A 0, B 1, C 2, D 3, E 4)
    (A 0, B 1, C 2, D 3, E 4, F 5)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11)
}
