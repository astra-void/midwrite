//! Types shared by the report and the PostgreSQL oracle (available without
//! the `postgres` feature so that reports can always be rendered).

use serde::{Deserialize, Serialize};

/// One comparison of a model prediction with what PostgreSQL did.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    /// 1-based index of the model transition.
    pub transition: usize,
    pub actor: String,
    pub sql: String,
    pub predicted: String,
    pub observed: String,
    pub agree: bool,
}
