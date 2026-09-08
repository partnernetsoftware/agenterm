//! Deliberately incompatible fixture for the launcher's fail-closed court.

#[unsafe(no_mangle)]
pub extern "C" fn agenterm_cu_process_main_abi_version() -> u32 {
    u32::MAX
}
