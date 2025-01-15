// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::bindings::exports::wasi::clocks::wall_clock::Datetime;
use crate::bindings::exports::wasi::filesystem::types::{
    Advice, DescriptorBorrow, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, ErrorBorrow, ErrorCode, Filesize, InputStream, MetadataHashValue,
    NewTimestamp, OpenFlags, OutputStream, PathFlags,
};
use crate::bindings::golem::api::durability::{observe_function_call, DurableFunctionType};
use crate::bindings::wasi::filesystem::types::filesystem_error_code;
use crate::durability::Durability;
use crate::wrappers::io::error::WrappedError;
use crate::wrappers::{SerializableError, SerializableFileTimes};
use metrohash::MetroHash128;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::Hasher;
use std::mem::transmute;
use std::path::PathBuf;

impl From<crate::bindings::wasi::filesystem::types::ErrorCode> for ErrorCode {
    fn from(value: crate::bindings::wasi::filesystem::types::ErrorCode) -> Self {
        unsafe { transmute(value) }
    }
}

pub struct WrappedDescriptor {
    pub descriptor: crate::bindings::wasi::filesystem::types::Descriptor,
    pub path: PathBuf,
}

impl crate::bindings::exports::wasi::filesystem::types::GuestDescriptor for WrappedDescriptor {
    fn read_via_stream(&self, offset: Filesize) -> Result<InputStream, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "read_via_stream");
        let input_stream = self.descriptor.read_via_stream(offset)?;
        Ok(InputStream::new(
            crate::wrappers::io::streams::WrappedInputStream {
                input_stream,
                is_incoming_http_body_stream: false,
            },
        ))
    }

    fn write_via_stream(&self, offset: Filesize) -> Result<OutputStream, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "write_via_stream");
        let output_stream = self.descriptor.write_via_stream(offset)?;
        Ok(OutputStream::new(
            crate::wrappers::io::streams::WrappedOutputStream { output_stream },
        ))
    }

    fn append_via_stream(&self) -> Result<OutputStream, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "append_via_stream");
        let output_stream = self.descriptor.append_via_stream()?;
        Ok(OutputStream::new(
            crate::wrappers::io::streams::WrappedOutputStream { output_stream },
        ))
    }

    fn advise(&self, offset: Filesize, length: Filesize, advice: Advice) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "advise");
        let advice = unsafe { transmute(advice) };
        Ok(self.descriptor.advise(offset, length, advice)?)
    }

    fn sync_data(&self) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "sync_data");
        Ok(self.descriptor.sync_data()?)
    }

    fn get_flags(&self) -> Result<DescriptorFlags, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "get_flags");
        let descriptor_flags = self.descriptor.get_flags()?;
        let descriptor_flags = unsafe { transmute(descriptor_flags) };
        Ok(descriptor_flags)
    }

    fn get_type(&self) -> Result<DescriptorType, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "get_type");
        let descriptor_type = self.descriptor.get_type()?;
        let descriptor_type = unsafe { transmute(descriptor_type) };
        Ok(descriptor_type)
    }

    fn set_size(&self, size: Filesize) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "set_size");
        Ok(self.descriptor.set_size(size)?)
    }

    fn set_times(
        &self,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "set_times");

        let data_access_timestamp = unsafe { transmute(data_access_timestamp) };
        let data_modification_timestamp = unsafe { transmute(data_modification_timestamp) };
        Ok(self
            .descriptor
            .set_times(data_access_timestamp, data_modification_timestamp)?)
    }

    fn read(&self, length: Filesize, offset: Filesize) -> Result<(Vec<u8>, bool), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "read");
        Ok(self.descriptor.read(length, offset)?)
    }

    fn write(&self, buffer: Vec<u8>, offset: Filesize) -> Result<Filesize, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "write");
        Ok(self.descriptor.write(&buffer, offset)?)
    }

    fn read_directory(&self) -> Result<DirectoryEntryStream, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "read_directory");
        let stream = self.descriptor.read_directory()?;
        // Iterating through the whole stream to make sure we have a stable order
        let mut entries = Vec::new();
        while let Some(entry) = stream.read_directory_entry()? {
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.name.clone());
        let entries = VecDeque::from(entries);

        Ok(DirectoryEntryStream::new(StableDirectoryEntryStream::new(
            entries,
        )))
    }

    fn sync(&self) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "sync");
        Ok(self.descriptor.sync()?)
    }

    fn create_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "create_directory_at");
        Ok(self.descriptor.create_directory_at(&path)?)
    }

    fn stat(&self) -> Result<DescriptorStat, ErrorCode> {
        let durability = Durability::<SerializableFileTimes, SerializableError>::new(
            "filesystem::types::descriptor",
            "stat",
            DurableFunctionType::ReadLocal,
        );

        let mut stat = self.descriptor.stat()?;
        stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it

        let times = if durability.is_live() {
            durability.persist_infallible(
                self.path.to_string_lossy().to_string(),
                SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                },
            )
        } else {
            durability.replay_infallible()
        };

        let accessed = times
            .data_access_timestamp
            .clone()
            .map(|t| crate::bindings::wasi::filesystem::types::NewTimestamp::Timestamp(t.into()))
            .unwrap_or(crate::bindings::wasi::filesystem::types::NewTimestamp::NoChange);
        let modified = times
            .data_modification_timestamp
            .clone()
            .map(|t| crate::bindings::wasi::filesystem::types::NewTimestamp::Timestamp(t.into()))
            .unwrap_or(crate::bindings::wasi::filesystem::types::NewTimestamp::NoChange);

        self.descriptor.set_times(accessed, modified)?;

        stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
        stat.data_modification_timestamp = times.data_modification_timestamp.map(|t| t.into());

        let stat = unsafe { transmute(stat) };
        Ok(stat)
    }

    fn stat_at(&self, path_flags: PathFlags, path: String) -> Result<DescriptorStat, ErrorCode> {
        let durability = Durability::<SerializableFileTimes, SerializableError>::new(
            "filesystem::types::descriptor",
            "stat_at",
            DurableFunctionType::ReadLocal,
        );

        let full_path = self.path.join(&path);

        let path_flags = unsafe { transmute(path_flags) };
        let mut stat = self.descriptor.stat_at(path_flags, &path)?;
        stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it

        let times = if durability.is_live() {
            durability.persist_infallible(
                full_path.to_string_lossy().to_string(),
                SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                },
            )
        } else {
            durability.replay_infallible()
        };

        let accessed = times
            .data_access_timestamp
            .clone()
            .map(|t| crate::bindings::wasi::filesystem::types::NewTimestamp::Timestamp(t.into()))
            .unwrap_or(crate::bindings::wasi::filesystem::types::NewTimestamp::NoChange);
        let modified = times
            .data_modification_timestamp
            .clone()
            .map(|t| crate::bindings::wasi::filesystem::types::NewTimestamp::Timestamp(t.into()))
            .unwrap_or(crate::bindings::wasi::filesystem::types::NewTimestamp::NoChange);

        self.descriptor
            .set_times_at(path_flags, &path, accessed, modified)?;

        stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
        stat.data_modification_timestamp = times.data_modification_timestamp.map(|t| t.into());

        let stat = unsafe { transmute(stat) };
        Ok(stat)
    }

    fn set_times_at(
        &self,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "set_times_at");

        let path_flags = unsafe { transmute(path_flags) };
        let data_access_timestamp = unsafe { transmute(data_access_timestamp) };
        let data_modification_timestamp = unsafe { transmute(data_modification_timestamp) };

        Ok(self.descriptor.set_times_at(
            path_flags,
            &path,
            data_access_timestamp,
            data_modification_timestamp,
        )?)
    }

    fn link_at(
        &self,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "link_at");
        let old_path_flags = unsafe { transmute(old_path_flags) };
        let new_descriptor = &new_descriptor.get::<WrappedDescriptor>().descriptor;
        Ok(self
            .descriptor
            .link_at(old_path_flags, &old_path, new_descriptor, &new_path)?)
    }

    fn open_at(
        &self,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<crate::bindings::exports::wasi::filesystem::types::Descriptor, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "open_at");
        let path_flags = unsafe { transmute(path_flags) };
        let open_flags = unsafe { transmute(open_flags) };
        let flags = unsafe { transmute(flags) };
        let descriptor = self
            .descriptor
            .open_at(path_flags, &path, open_flags, flags)?;
        Ok(
            crate::bindings::exports::wasi::filesystem::types::Descriptor::new(WrappedDescriptor {
                descriptor,
                path: self.path.join(&path),
            }),
        )
    }

    fn readlink_at(&self, path: String) -> Result<String, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "readlink_at");
        Ok(self.descriptor.readlink_at(&path)?)
    }

    fn remove_directory_at(&self, path: String) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "remove_directory_at");
        Ok(self.descriptor.remove_directory_at(&path)?)
    }

    fn rename_at(
        &self,
        old_path: String,
        new_descriptor: DescriptorBorrow<'_>,
        new_path: String,
    ) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "rename_at");
        Ok(self.descriptor.rename_at(
            &old_path,
            &new_descriptor.get::<WrappedDescriptor>().descriptor,
            &new_path,
        )?)
    }

    fn symlink_at(&self, old_path: String, new_path: String) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "symlink_at");
        Ok(self.descriptor.symlink_at(&old_path, &new_path)?)
    }

    fn unlink_file_at(&self, path: String) -> Result<(), ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        Ok(self.descriptor.unlink_file_at(&path)?)
    }

    fn is_same_object(&self, other: DescriptorBorrow<'_>) -> bool {
        observe_function_call("filesystem::types::descriptor", "is_same_object");
        self.descriptor
            .is_same_object(&other.get::<WrappedDescriptor>().descriptor)
    }

    fn metadata_hash(&self) -> Result<MetadataHashValue, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "metadata_hash");

        // Using the WASI stat function as it guarantees the file times are preserved
        let metadata = self.stat()?;
        Ok(calculate_metadata_hash(&metadata))
    }

    fn metadata_hash_at(
        &self,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, ErrorCode> {
        observe_function_call("filesystem::types::descriptor", "metadata_hash_at");
        // Using the WASI stat_at function as it guarantees the file times are preserved
        let metadata = self.stat_at(path_flags, path)?;

        Ok(calculate_metadata_hash(&metadata))
    }
}

impl Drop for WrappedDescriptor {
    fn drop(&mut self) {
        observe_function_call("filesystem::types::descriptor", "drop");
    }
}

pub struct StableDirectoryEntryStream {
    pub entries: RefCell<VecDeque<crate::bindings::wasi::filesystem::types::DirectoryEntry>>,
}

impl StableDirectoryEntryStream {
    pub fn new(
        entries: VecDeque<crate::bindings::wasi::filesystem::types::DirectoryEntry>,
    ) -> Self {
        Self {
            entries: RefCell::new(entries),
        }
    }
}

impl crate::bindings::exports::wasi::filesystem::types::GuestDirectoryEntryStream
    for StableDirectoryEntryStream
{
    fn read_directory_entry(&self) -> Result<Option<DirectoryEntry>, ErrorCode> {
        observe_function_call(
            "filesystem::types::directory_entry_stream",
            "read_directory_entry",
        );
        let entry = self.entries.borrow_mut().pop_front();
        let entry = unsafe { transmute(entry) };
        Ok(entry)
    }
}

impl Drop for StableDirectoryEntryStream {
    fn drop(&mut self) {
        observe_function_call("filesystem::types::directory_entry_stream", "drop");
    }
}

impl crate::bindings::exports::wasi::filesystem::types::Guest for crate::Component {
    type Descriptor = WrappedDescriptor;
    type DirectoryEntryStream = StableDirectoryEntryStream;

    fn filesystem_error_code(err: ErrorBorrow<'_>) -> Option<ErrorCode> {
        let error = &err.get::<WrappedError>().error;
        let code = filesystem_error_code(error);
        unsafe { transmute(code) }
    }
}

fn calculate_metadata_hash(meta: &DescriptorStat) -> MetadataHashValue {
    let mut hasher = MetroHash128::new();

    let modified = meta.data_modification_timestamp.unwrap_or(Datetime {
        seconds: 0,
        nanoseconds: 0,
    });
    hasher.write_u64(modified.seconds);
    hasher.write_u32(modified.nanoseconds);
    hasher.write_u64(meta.size);

    let (lower, upper) = hasher.finish128();
    MetadataHashValue { lower, upper }
}
