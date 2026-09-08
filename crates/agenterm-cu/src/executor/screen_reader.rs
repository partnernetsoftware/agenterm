use serde_json::Value;

use crate::reply::CuError;
use crate::screen_reader_observe::status_payload;

pub(super) fn screen_reader_status_payload() -> Result<Value, CuError> {
    status_payload()
}
