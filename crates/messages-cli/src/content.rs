use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Body {
    pub text: Option<String>,
    pub status: BodyStatus,
}

use crate::BodyStatus;

pub(crate) fn decode_body(text: Option<String>, attributed: Option<Vec<u8>>) -> Body {
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        return Body {
            text: Some(text),
            status: BodyStatus::Plain,
        };
    }
    let Some(attributed) = attributed.filter(|bytes| !bytes.is_empty()) else {
        return Body {
            text: None,
            status: BodyStatus::Empty,
        };
    };
    match TypedString::decode(&attributed) {
        Some(text) => Body {
            text: Some(text),
            status: BodyStatus::Decoded,
        },
        None => Body {
            text: None,
            status: BodyStatus::Unsupported,
        },
    }
}

// Read only the root attributed string's NSString, using typedstream's actual
// class, shared-string, and length records. Attribute dictionaries are not
// needed for plain text. Unknown layouts remain explicit unsupported content;
// no Objective-C classes from an arbitrary database are ever instantiated.
struct TypedString<'a> {
    bytes: &'a [u8],
    offset: usize,
    little_endian: bool,
    strings: Vec<&'a [u8]>,
    classes: Vec<&'a [u8]>,
    // Class references share an index space with objects. None reserves an
    // object slot; it must never be accepted as a class reference.
    references: Vec<Option<usize>>,
}

impl<'a> TypedString<'a> {
    fn decode(bytes: &'a [u8]) -> Option<String> {
        let signature = bytes.get(..13)?;
        if signature.get(..2)? != [4, 11] {
            return None;
        }
        let little_endian = match signature.get(2..)? {
            b"streamtyped" => true,
            b"typedstream" => false,
            _ => return None,
        };
        let mut reader = Self {
            bytes,
            offset: 13,
            little_endian,
            strings: Vec::new(),
            classes: Vec::new(),
            references: Vec::new(),
        };
        if reader.integer(false)? != 1000 || reader.shared_string()? != b"@" {
            return None;
        }
        let root = reader.object_class()?;
        if !matches!(root, b"NSAttributedString" | b"NSMutableAttributedString") {
            return None;
        }
        if reader.shared_string()? != b"@" {
            return None;
        }
        let string_class = reader.object_class()?;
        if !matches!(string_class, b"NSString" | b"NSMutableString") || reader.shared_string()? != b"+" {
            return None;
        }
        let text = std::str::from_utf8(reader.unshared_string()?).ok()?.to_owned();
        // A complete string object is required. In particular, do not accept a
        // length prefix that consumes bytes from the trailing attribute data.
        if reader.byte()? != 0x86 {
            return None;
        }
        Some(text)
    }

    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.offset.checked_add(length)?;
        let bytes = self.bytes.get(self.offset..end)?;
        self.offset = end;
        Some(bytes)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn integer(&mut self, signed: bool) -> Option<i64> {
        let head = self.byte()?;
        match head {
            0x81 => {
                let bytes: [u8; 2] = self.take(2)?.try_into().ok()?;
                let value = if self.little_endian {
                    u16::from_le_bytes(bytes)
                } else {
                    u16::from_be_bytes(bytes)
                };
                Some(if signed {
                    i64::from(value as i16)
                } else {
                    i64::from(value)
                })
            }
            0x82 => {
                let bytes: [u8; 4] = self.take(4)?.try_into().ok()?;
                let value = if self.little_endian {
                    u32::from_le_bytes(bytes)
                } else {
                    u32::from_be_bytes(bytes)
                };
                Some(if signed {
                    i64::from(value as i32)
                } else {
                    i64::from(value)
                })
            }
            0x80..=0x91 => None,
            _ => Some(if signed { i64::from(head as i8) } else { i64::from(head) }),
        }
    }

    fn unshared_string(&mut self) -> Option<&'a [u8]> {
        let length = usize::try_from(self.integer(false)?).ok()?;
        self.take(length)
    }

    fn shared_string(&mut self) -> Option<&'a [u8]> {
        if self.bytes.get(self.offset)? == &0x84 {
            self.offset += 1;
            let string = self.unshared_string()?;
            self.strings.push(string);
            Some(string)
        } else {
            let reference = usize::try_from(self.integer(true)?.checked_add(110)?).ok()?;
            self.strings.get(reference).copied()
        }
    }

    fn object_class(&mut self) -> Option<&'a [u8]> {
        if self.byte()? != 0x84 {
            return None;
        }
        self.references.push(None);
        let mut first = None;
        // Foundation's supported hierarchies here are at most three classes.
        // A small explicit bound also rejects cyclic/malformed inputs.
        for _ in 0..8 {
            match self.bytes.get(self.offset)? {
                0x84 => {
                    self.offset += 1;
                    let name = self.shared_string()?;
                    let version = self.integer(true)?;
                    match name {
                        b"NSObject" if version == 0 => {}
                        b"NSString" | b"NSMutableString" if version == 1 => {}
                        b"NSAttributedString" | b"NSMutableAttributedString" if version == 0 => {}
                        _ => return None,
                    }
                    first.get_or_insert(name);
                    self.references.push(Some(self.classes.len()));
                    self.classes.push(name);
                }
                0x85 => {
                    self.offset += 1;
                    return first;
                }
                _ => {
                    let reference = usize::try_from(self.integer(true)?.checked_add(110)?).ok()?;
                    let class = self.references.get(reference)?.as_ref()?;
                    let name = self.classes.get(*class).copied()?;
                    return first.or(Some(name));
                }
            }
        }
        None
    }
}

pub(crate) fn normalize_handle(handle: &str) -> String {
    let handle = handle.trim();
    if handle.contains('@') {
        return handle.to_lowercase();
    }
    if handle
        .chars()
        .all(|character| character.is_ascii_digit() || "+()- .".contains(character))
    {
        let digits = handle.chars().filter(char::is_ascii_digit).collect::<String>();
        if !digits.is_empty() {
            return if handle.starts_with('+') {
                format!("+{digits}")
            } else {
                digits
            };
        }
    }
    handle.to_lowercase()
}

pub(crate) fn contact_names() -> HashMap<String, String> {
    #[cfg(target_os = "macos")]
    {
        macos::contact_names()
    }
    #[cfg(not(target_os = "macos"))]
    {
        HashMap::new()
    }
}

pub(crate) fn contacts_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::contacts_available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub(crate) fn open_attachment(root: &std::path::Path, relative: &std::path::Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::{
            ffi::CString,
            os::{fd::FromRawFd, unix::ffi::OsStrExt},
        };

        extern "C" {
            fn messages_open_attachment(
                root: *const std::os::raw::c_char,
                relative: *const std::os::raw::c_char,
            ) -> std::os::raw::c_int;
        }
        let root = CString::new(root.as_os_str().as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let relative = CString::new(relative.as_os_str().as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        // The helper transfers exactly one regular-file descriptor on success,
        // and closes all intermediate descriptors on both success and failure.
        let descriptor = unsafe { messages_open_attachment(root.as_ptr(), relative.as_ptr()) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
    }
    #[cfg(not(unix))]
    {
        let _ = (root, relative);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "attachment reading requires Unix directory-relative file access",
        ))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::HashMap;

    use serde::Deserialize;

    use crate::content::normalize_handle;

    extern "C" {
        fn messages_contacts_available() -> bool;
        fn messages_contact_names(length: *mut usize) -> *mut u8;
        fn messages_free_contacts(bytes: *mut u8);
    }

    pub(super) fn contacts_available() -> bool {
        // This checks the existing permission and does not request access.
        unsafe { messages_contacts_available() }
    }

    #[derive(Deserialize)]
    struct ContactName {
        handle: String,
        name: String,
    }

    pub(super) fn contact_names() -> HashMap<String, String> {
        let mut length = 0;
        // The helper returns a malloc-owned buffer, never an Objective-C object.
        // Copy before releasing it, and release it on every non-null path.
        let bytes = unsafe {
            let pointer = messages_contact_names(&mut length);
            if pointer.is_null() {
                return HashMap::new();
            }
            let result = if length <= 16 * 1024 * 1024 {
                Some(std::slice::from_raw_parts(pointer, length).to_vec())
            } else {
                None
            };
            messages_free_contacts(pointer);
            result
        };
        let Some(contacts) = bytes.and_then(|bytes| serde_json::from_slice::<Vec<ContactName>>(&bytes).ok()) else {
            return HashMap::new();
        };
        let mut names = HashMap::new();
        for contact in contacts {
            names.entry(normalize_handle(&contact.handle)).or_insert(contact.name);
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        content::{decode_body, normalize_handle},
        BodyStatus,
    };

    #[test]
    fn plain_text_takes_precedence_and_empty_is_explicit() {
        let body = decode_body(Some("Hello".into()), Some(vec![0xff]));
        assert_eq!(body.text.as_deref(), Some("Hello"));
        assert_eq!(body.status, BodyStatus::Plain);
        assert_eq!(decode_body(None, None).status, BodyStatus::Empty);
        assert_eq!(
            decode_body(Some(String::new()), Some(Vec::new())).status,
            BodyStatus::Empty
        );
        assert_eq!(decode_body(None, Some(vec![0xff])).status, BodyStatus::Unsupported);
    }

    #[test]
    fn decodes_foundation_archived_strings_with_unicode_and_long_lengths() {
        let fixtures: &[(&[u8], String)] = &[
            (
                include_bytes!("../tests/fixtures/plain.typedstream"),
                "Hello from Messages 👋".into(),
            ),
            (
                include_bytes!("../tests/fixtures/long.typedstream"),
                "A🙂\0Z".repeat(400),
            ),
            (include_bytes!("../tests/fixtures/empty.typedstream"), String::new()),
            (
                include_bytes!("../tests/fixtures/mutable.typedstream"),
                "Styled text".into(),
            ),
        ];
        for (bytes, expected) in fixtures {
            let body = decode_body(None, Some(bytes.to_vec()));
            assert_eq!(body.status, BodyStatus::Decoded);
            assert_eq!(body.text.as_ref(), Some(expected));
        }
    }

    #[test]
    fn truncated_or_arbitrary_streams_never_become_text() {
        let fixture = include_bytes!("../tests/fixtures/plain.typedstream");
        for length in 0..95.min(fixture.len()) {
            let body = decode_body(None, Some(fixture[..length].to_vec()));
            assert_ne!(
                body.status,
                BodyStatus::Decoded,
                "accepted truncated stream at {length}"
            );
        }
        let mut invalid = fixture.to_vec();
        invalid[0] = 3;
        assert_eq!(decode_body(None, Some(invalid)).status, BodyStatus::Unsupported);
        assert_eq!(
            decode_body(None, Some(b"noise\x01+text\x86\x84".to_vec())).status,
            BodyStatus::Unsupported
        );

        let mut invalid_utf8 = fixture.to_vec();
        invalid_utf8[74] = 0xff;
        assert_eq!(decode_body(None, Some(invalid_utf8)).status, BodyStatus::Unsupported);
    }

    #[test]
    fn handles_are_normalized_without_guessing_a_country() {
        assert_eq!(normalize_handle(" +1 (555) 555-0123 "), "+15555550123");
        assert_eq!(normalize_handle("555-0123"), "5550123");
        assert_eq!(normalize_handle("Person@Example.COM"), "person@example.com");
        assert_eq!(normalize_handle("Alex Smith"), "alex smith");
    }

    #[cfg(unix)]
    #[test]
    fn attachment_open_rejects_symlinks_traversal_and_special_files() {
        use std::{fs, io::Read, os::unix::fs::symlink, path::Path, process::Command};

        use crate::content::open_attachment;

        let directory = tempfile::tempdir().expect("fixture operation succeeds");
        let root = directory
            .path()
            .canonicalize()
            .expect("fixture operation succeeds")
            .join("Attachments");
        fs::create_dir_all(root.join("nested")).expect("fixture operation succeeds");
        fs::write(root.join("nested/file"), b"attachment bytes").expect("fixture operation succeeds");
        fs::write(directory.path().join("private"), b"private bytes").expect("fixture operation succeeds");
        let mut file = open_attachment(&root, Path::new("nested/file")).expect("fixture operation succeeds");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("fixture operation succeeds");
        assert_eq!(bytes, b"attachment bytes");
        assert!(open_attachment(&root, Path::new("../private")).is_err());
        assert!(open_attachment(&root, &directory.path().join("private")).is_err());
        assert!(open_attachment(&root, Path::new("nested")).is_err());
        symlink(directory.path().join("private"), root.join("file-link")).expect("fixture operation succeeds");
        symlink(directory.path(), root.join("directory-link")).expect("fixture operation succeeds");
        assert!(open_attachment(&root, Path::new("file-link")).is_err());
        assert!(open_attachment(&root, Path::new("directory-link/private")).is_err());
        assert!(Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .expect("fixture operation succeeds")
            .success());
        assert!(open_attachment(&root, Path::new("pipe")).is_err());
        let alias = root.parent().expect("fixture operation succeeds").join("alias");
        symlink(&root, &alias).expect("fixture operation succeeds");
        assert!(open_attachment(&alias, Path::new("nested/file")).is_err());
    }
}
