use crate::login_session::{LoginSessionError, LoginSessionErrorKind, LoginSessionInventory};

pub(crate) fn inventory() -> Result<LoginSessionInventory, LoginSessionError> {
    Err(unsupported())
}

pub(crate) fn lock_console() -> Result<(), LoginSessionError> {
    Err(unsupported())
}

fn unsupported() -> LoginSessionError {
    LoginSessionError::new(
        LoginSessionErrorKind::Unsupported,
        "login-session inventory and console locking are unsupported on this host",
    )
}
