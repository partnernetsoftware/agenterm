//! Unix filesystem entry classification.

use std::fs::Metadata;

use crate::filesystem_entry::NativeEntryDetails;

pub(crate) fn metadata_is_link_like(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn native_details(metadata: &Metadata) -> NativeEntryDetails {
    use std::os::unix::fs::MetadataExt as _;

    NativeEntryDetails {
        identity: Some(format!("unix:{:x}:{:x}", metadata.dev(), metadata.ino())),
        unix_mode: Some(metadata.mode()),
        unix_uid: Some(metadata.uid()),
        unix_gid: Some(metadata.gid()),
        windows_attributes: None,
    }
}
