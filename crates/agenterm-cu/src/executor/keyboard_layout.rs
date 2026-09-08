use serde_json::Value;

use crate::keyboard_layout_observe::status_payload;
use crate::reply::CuError;

pub(super) fn keyboard_layout_status_payload() -> Result<Value, CuError> {
    status_payload()
}
