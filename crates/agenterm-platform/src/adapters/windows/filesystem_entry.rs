//! Windows filesystem entry classification.

use std::{fs::Metadata, os::windows::fs::MetadataExt as _};

use crate::filesystem_entry::NativeEntryDetails;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

pub(crate) fn metadata_is_link_like(metadata: &Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub(crate) fn native_details(metadata: &Metadata) -> NativeEntryDetails {
    NativeEntryDetails {
        // Stable Windows object identity requires an opened handle and the
        // separate `file-identity` facade. MetadataExt's by-handle identity
        // accessors are unstable; do not replace them with a path spelling.
        identity: None,
        unix_mode: None,
        unix_uid: None,
        unix_gid: None,
        windows_attributes: Some(metadata.file_attributes()),
    }
}
