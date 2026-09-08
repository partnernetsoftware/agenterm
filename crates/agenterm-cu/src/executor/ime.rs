use serde_json::Value;

use crate::ime_observe::status_payload;
use crate::reply::CuError;

pub(super) fn ime_status_payload() -> Result<Value, CuError> {
    status_payload()
}
