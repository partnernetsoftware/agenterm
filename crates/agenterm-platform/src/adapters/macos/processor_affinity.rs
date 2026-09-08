use crate::contract::processor_affinity::{
    ProcessorAffinityError, ProcessorAffinityErrorKind, ProcessorAffinityFacts,
};

pub(crate) fn current_process() -> Result<ProcessorAffinityFacts, ProcessorAffinityError> {
    unsupported()
}

pub(crate) fn process(_pid: u32) -> Result<ProcessorAffinityFacts, ProcessorAffinityError> {
    unsupported()
}

fn unsupported() -> Result<ProcessorAffinityFacts, ProcessorAffinityError> {
    Err(ProcessorAffinityError::new(
        ProcessorAffinityErrorKind::Unsupported,
        "macOS affinity tags are advisory and do not expose an exact process allowed-CPU set",
    ))
}
