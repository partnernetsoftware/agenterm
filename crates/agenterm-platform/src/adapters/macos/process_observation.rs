use crate::contract::process_observation::{ParentProcessObservation, ProcessObservation};

pub(crate) fn observe(pid: u32) -> ProcessObservation {
    let Ok(pid) = i32::try_from(pid) else {
        return ProcessObservation::Dead {
            reason: "process_id_out_of_range".to_owned(),
        };
    };
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => ProcessObservation::Dead {
                reason: "process_not_found".to_owned(),
            },
            Some(libc::EPERM) | Some(libc::EACCES) => ProcessObservation::Unknown {
                reason: "process_access_denied".to_owned(),
            },
            _ => ProcessObservation::Unknown {
                reason: format!("process_identity_read_failed:{error}"),
            },
        };
    }
    ProcessObservation::Live {
        start_identity: Some(format!(
            "macos-start-time:{}.{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        )),
    }
}

pub(crate) fn parent(pid: u32) -> ParentProcessObservation {
    let Ok(pid) = i32::try_from(pid) else {
        return ParentProcessObservation::Dead {
            reason: "process_id_out_of_range".to_owned(),
        };
    };
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if read != size {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ESRCH) => ParentProcessObservation::Dead {
                reason: "process_not_found".to_owned(),
            },
            Some(libc::EPERM) | Some(libc::EACCES) => ParentProcessObservation::Unknown {
                reason: "process_access_denied".to_owned(),
            },
            _ => ParentProcessObservation::Unknown {
                reason: format!("process_parent_read_failed:{error}"),
            },
        };
    }
    ParentProcessObservation::Live {
        parent_id: info.pbi_ppid,
    }
}
