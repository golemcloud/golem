//! Gives the last step of an edit in place to the file editor of the call.
//!
//! `sed -i` writes the new content of a file to a copy in the same directory, and then it renames
//! the copy over the file. The rename is a call into libc, in code that this crate does not own.
//! The build wraps that call (see `devices`), and while a run edits in place ([`edit`]), this
//! module answers it: the file editor changes the content of the file, and the copy is removed.
//!
//! A rename puts a new file in the place of the old one, and the file editor changes the old
//! file. A script can see that difference only through another name of the file or through a
//! descriptor that is open on the file, so such a file is left to the rename. On a host file
//! system that ignores the case of names, one more difference stays: a rename gives the file
//! the name as the command writes it, and the file editor keeps the name that the file has.
use std::cell::RefCell;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use super::devices;
use crate::commands::FileEditor;

/// The file editor of a run that edits in place, and the descriptors that were open before it.
struct Editing {
    editor: Arc<dyn FileEditor>,
    open_before: Vec<i32>,
}

thread_local! {
    static EDITING: RefCell<Option<Rc<Editing>>> = const { RefCell::new(None) };
}

/// Ends the use of the file editor when it drops.
pub(crate) struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        EDITING.with_borrow_mut(|editing| *editing = None);
    }
}

/// Lets `editor` make each replacement of a file by its edited copy, until the guard drops. The
/// guard must drop before the task gives way to another part of the script.
pub(crate) fn edit(editor: Arc<dyn FileEditor>) -> Guard {
    let editing = Editing {
        editor,
        open_before: devices::open_descriptors(),
    };
    EDITING.with_borrow_mut(|slot| *slot = Some(Rc::new(editing)));
    Guard
}

/// Answers the rename of `copy` over `file`: `true` when the file editor has given `file` the
/// content of `copy` and `copy` is removed, `false` when the rename must be made.
pub(crate) fn replaces(copy: &CStr, file: &CStr) -> bool {
    let Some(editing) = EDITING.with_borrow(Clone::clone) else {
        return false;
    };
    let (Some(copy_path), Some(file_path)) = (absolute(copy), absolute(file)) else {
        return false;
    };
    let (Some(new), Some(old)) = (devices::status(copy), devices::status(file)) else {
        return false;
    };
    let regular = |status: &libc::stat| status.st_mode & libc::S_IFMT == libc::S_IFREG;
    let limit = i64::try_from(editing.editor.max_len()).unwrap_or(i64::MAX);
    // The rename puts a regular file in the place of a symbolic link, and it leaves the other
    // names of a file with the old content.
    if !regular(&old) || old.st_nlink != 1 || new.st_size > limit || old.st_size > limit {
        return false;
    }
    // A descriptor from before the run keeps the old content when the file is replaced.
    let open = editing.open_before.iter().any(|descriptor| {
        devices::descriptor_status(*descriptor)
            .is_some_and(|status| (status.st_dev, status.st_ino) == (old.st_dev, old.st_ino))
    });
    if open {
        return false;
    }
    let (Ok(before), Ok(after)) = (std::fs::read(&file_path), std::fs::read(&copy_path)) else {
        return false;
    };
    editing.editor.edit(&file_path, &before, &after)
        && devices::status(file).is_some_and(|status| {
            regular(&status)
                && u64::try_from(status.st_size).ok() == u64::try_from(after.len()).ok()
        })
        && devices::remove(copy)
}

/// `path` as an absolute path: a relative one is in the working directory of the process, which
/// is the directory of the shell while a command runs. The file editor takes UTF-8 paths only.
fn absolute(path: &CStr) -> Option<PathBuf> {
    let path = Path::new(path.to_str().ok()?);
    if path.is_absolute() {
        return Some(path.to_owned());
    }
    Some(std::env::current_dir().ok()?.join(path))
}
