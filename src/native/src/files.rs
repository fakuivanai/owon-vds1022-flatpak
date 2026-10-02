//! FileChooser requests preserve the document portal's granted URIs.
//! Protocol: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html

use crate::portal::{Portal, PortalError};
use std::{borrow::Cow, collections::HashMap, error::Error, fmt};
use zbus::zvariant::{OwnedValue, Type, Value};

type PortalFilter = (String, Vec<(u32, String)>);

#[derive(Debug)]
pub enum FileError {
    InvalidRequest(&'static str),
    InvalidResponse(&'static str),
    Portal(PortalError),
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) | Self::InvalidResponse(message) => f.write_str(message),
            Self::Portal(error) => error.fmt(f),
        }
    }
}

impl Error for FileError {}

#[derive(Clone, Debug)]
pub struct Filter {
    label: String,
    patterns: Vec<String>,
}

impl Filter {
    pub fn new(label: String, patterns: Vec<String>) -> Result<Self, FileError> {
        validate_string(&label)?;
        if patterns.is_empty() {
            return Err(FileError::InvalidRequest("Missing filter patterns"));
        }
        for pattern in &patterns {
            validate_string(pattern)?;
        }
        Ok(Self { label, patterns })
    }

    fn matches(&self, returned: &PortalFilter) -> bool {
        self.label == returned.0
            && self.patterns.len() == returned.1.len()
            && self
                .patterns
                .iter()
                .zip(&returned.1)
                .all(|(expected, (kind, pattern))| {
                    *kind == 0 && normalize_suffix_glob(expected) == normalize_suffix_glob(pattern)
                })
    }
}

#[derive(Debug)]
enum Action {
    Save { name: String },
    Open { multiple: bool, directory: bool },
}

#[derive(Debug)]
pub struct FileRequest {
    title: String,
    folder: String,
    accept_label: String,
    filters: Vec<Filter>,
    selected_filter: usize,
    action: Action,
}

impl FileRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        save: bool,
        title: String,
        name: String,
        folder: String,
        accept_label: String,
        filters: Vec<Filter>,
        selected_filter: usize,
        multiple: bool,
        directory: bool,
    ) -> Result<Self, FileError> {
        for text in [&title, &name, &folder, &accept_label] {
            validate_string(text)?;
        }
        if filters.is_empty() || selected_filter >= filters.len() {
            return Err(FileError::InvalidRequest("Invalid file filters"));
        }
        let action = if save {
            Action::Save { name }
        } else {
            Action::Open {
                multiple,
                directory,
            }
        };
        Ok(Self {
            title,
            folder,
            accept_label,
            filters,
            selected_filter,
            action,
        })
    }

    fn selection(&self, mut results: HashMap<String, OwnedValue>) -> Result<Selection, FileError> {
        let uris = results
            .remove("uris")
            .and_then(|value| Vec::<String>::try_from(value).ok())
            .filter(|uris| {
                !uris.is_empty() && (!matches!(self.action, Action::Save { .. }) || uris.len() == 1)
            })
            .ok_or(FileError::InvalidResponse(
                "File portal returned invalid file selection",
            ))?;
        let selected_filter = match results.remove("current_filter") {
            None => self.selected_filter,
            Some(value) => {
                // Check the complete signature before tuple extraction, including nested fields.
                if value.value_signature() != <PortalFilter as Type>::SIGNATURE {
                    return Err(FileError::InvalidResponse(
                        "File portal returned an invalid format",
                    ));
                }
                let returned = PortalFilter::try_from(value).map_err(|_| {
                    FileError::InvalidResponse("File portal returned an invalid format")
                })?;
                let mut matches = self
                    .filters
                    .iter()
                    .enumerate()
                    .filter(|(_, filter)| filter.matches(&returned));
                let selected = matches
                    .next()
                    .ok_or(FileError::InvalidResponse(
                        "File portal returned an unknown format",
                    ))?
                    .0;
                if matches.next().is_some() {
                    return Err(FileError::InvalidResponse(
                        "File portal returned an ambiguous format",
                    ));
                }
                selected
            }
        };
        Ok(Selection {
            uris,
            selected_filter,
        })
    }
}

#[derive(Debug)]
pub struct Selection {
    pub uris: Vec<String>,
    pub selected_filter: usize,
}

pub fn choose(portal: &Portal, request: FileRequest) -> Result<Option<Selection>, FileError> {
    let token = portal.token();
    let filters: Vec<_> = request
        .filters
        .iter()
        .map(|filter| {
            (
                filter.label.as_str(),
                filter
                    .patterns
                    .iter()
                    .map(|pattern| (0_u32, pattern.as_str()))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut options = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("modal", Value::from(true));
    options.insert(
        "current_filter",
        Value::from(filters[request.selected_filter].clone()),
    );
    options.insert("filters", Value::from(filters));
    if !request.accept_label.is_empty() {
        options.insert("accept_label", Value::from(request.accept_label.as_str()));
    }
    if !request.folder.is_empty() {
        let mut folder = request.folder.as_bytes().to_vec();
        folder.push(0);
        options.insert("current_folder", Value::from(folder));
    }
    let method = match &request.action {
        Action::Save { name } => {
            if !name.is_empty() {
                options.insert("current_name", Value::from(name.as_str()));
            }
            "SaveFile"
        }
        Action::Open {
            multiple,
            directory,
        } => {
            options.insert("multiple", Value::from(*multiple));
            options.insert("directory", Value::from(*directory));
            "OpenFile"
        }
    };
    match portal.request(
        "org.freedesktop.portal.FileChooser",
        method,
        &("", request.title.as_str(), options),
        &token,
    ) {
        Ok(response) => request.selection(response.results).map(Some),
        Err(PortalError::Cancelled) => Ok(None),
        Err(error) => Err(FileError::Portal(error)),
    }
}

fn validate_string(text: &str) -> Result<(), FileError> {
    if text.contains('\0') {
        Err(FileError::InvalidRequest(
            "File portal strings must not contain NUL",
        ))
    } else {
        Ok(())
    }
}

fn normalize_suffix_glob(pattern: &str) -> Cow<'_, str> {
    // KDE rewrites complete ASCII case-pair suffixes, *.[pP][nN][gG] -> *.png.
    // Match only that round trip; retain every other pattern exactly.
    // https://github.com/KDE/xdg-desktop-portal-kde/blob/v6.6.6/src/filechooser.cpp
    let bytes = pattern.as_bytes();
    if !bytes.starts_with(b"*.") || bytes.len() <= 2 || !(bytes.len() - 2).is_multiple_of(4) {
        return Cow::Borrowed(pattern);
    }
    let mut normalized = String::from("*.");
    for [open, first, second, close] in bytes[2..].as_chunks::<4>().0 {
        if *open != b'['
            || *close != b']'
            || !((first.is_ascii_lowercase() && second.is_ascii_uppercase())
                || (first.is_ascii_uppercase() && second.is_ascii_lowercase()))
            || !first.eq_ignore_ascii_case(second)
        {
            return Cow::Borrowed(pattern);
        }
        normalized.push(char::from(first.to_ascii_lowercase()));
    }
    Cow::Owned(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(label: &str, patterns: &[&str]) -> Filter {
        Filter::new(
            label.into(),
            patterns.iter().map(|pattern| (*pattern).into()).collect(),
        )
        .unwrap()
    }

    fn request(
        save: bool,
        filters: Vec<Filter>,
        selected: usize,
    ) -> Result<FileRequest, FileError> {
        FileRequest::new(
            save,
            "Captura ñ 💡".into(),
            "".into(),
            "".into(),
            "".into(),
            filters,
            selected,
            true,
            false,
        )
    }

    fn results(uris: Vec<&str>, selected: Option<PortalFilter>) -> HashMap<String, OwnedValue> {
        let mut results = HashMap::from([(
            "uris".into(),
            OwnedValue::try_from(Value::from(uris)).unwrap(),
        )]);
        if let Some(selected) = selected {
            results.insert(
                "current_filter".into(),
                OwnedValue::try_from(Value::from(selected)).unwrap(),
            );
        }
        results
    }

    #[test]
    fn normalize_only_complete_ascii_case_pairs() {
        assert_eq!(normalize_suffix_glob("*.[pP][Nn][gG]"), "*.png");
        for unchanged in [
            "*.png", "*.", "*.[pp]", "*.[pq]", "*.[pP]ng", "*.[pP]?", "*.[ñÑ]", "*.[a-z]", "x.[pP]",
        ] {
            assert_eq!(normalize_suffix_glob(unchanged), unchanged);
        }
    }

    #[test]
    fn reject_invalid_request_before_portal_access() {
        assert!(Filter::new("label".into(), vec![]).is_err());
        assert!(Filter::new("la\0bel".into(), vec!["*".into()]).is_err());
        assert!(Filter::new("label".into(), vec!["*\0".into()]).is_err());
        assert!(request(false, vec![], 0).is_err());
        assert!(request(false, vec![filter("PNG", &["*.png"])], 1).is_err());
        assert!(
            FileRequest::new(
                false,
                "\0".into(),
                "".into(),
                "".into(),
                "".into(),
                vec![filter("PNG", &["*.png"])],
                0,
                false,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn preserve_uri_order_and_initial_filter_without_returned_filter() {
        let request = request(
            false,
            vec![filter("PNG", &["*.png"]), filter("CSV", &["*.csv"])],
            1,
        )
        .unwrap();
        let selection = request
            .selection(results(
                vec![
                    "file:///tmp/se%C3%B1al%20%F0%9F%92%A1.csv",
                    "file:///tmp/second.csv",
                ],
                None,
            ))
            .unwrap();
        assert_eq!(selection.selected_filter, 1);
        assert_eq!(
            selection.uris,
            [
                "file:///tmp/se%C3%B1al%20%F0%9F%92%A1.csv",
                "file:///tmp/second.csv"
            ]
        );
    }

    #[test]
    fn reject_missing_empty_multiple_save_and_wrongly_typed_uris() {
        let request = request(true, vec![filter("PNG", &["*.png"])], 0).unwrap();
        for malformed in [
            HashMap::new(),
            results(vec![], None),
            results(vec!["file:///a", "file:///b"], None),
            HashMap::from([("uris".into(), OwnedValue::from(true))]),
        ] {
            assert!(request.selection(malformed).is_err());
        }
    }

    #[test]
    fn require_exact_label_pattern_order_and_kind() {
        let expected = filter("PNG", &["*.[pP][nN][gG]", "*.other"]);
        assert!(expected.matches(&(
            "PNG".into(),
            vec![(0, "*.png".into()), (0, "*.other".into())]
        )));
        for returned in [
            (
                "PNG other".into(),
                vec![(0, "*.png".into()), (0, "*.other".into())],
            ),
            (
                "PNG".into(),
                vec![(0, "*.other".into()), (0, "*.png".into())],
            ),
            (
                "PNG".into(),
                vec![(1, "*.png".into()), (0, "*.other".into())],
            ),
            ("PNG".into(), vec![(0, "*.png".into())]),
        ] {
            assert!(!expected.matches(&returned));
        }
    }

    #[test]
    fn reject_unknown_ambiguous_and_malformed_returned_filter() {
        let request = request(
            false,
            vec![
                filter("CSV", &["*.[cC][sS][vV]"]),
                filter("CSV", &["*.csv"]),
            ],
            0,
        )
        .unwrap();
        let ambiguous = request
            .selection(results(
                vec!["file:///a"],
                Some(("CSV".into(), vec![(0, "*.csv".into())])),
            ))
            .unwrap_err();
        assert!(ambiguous.to_string().contains("ambiguous"));
        let unknown = request
            .selection(results(
                vec!["file:///a"],
                Some(("CSV".into(), vec![(0, "*.xls".into())])),
            ))
            .unwrap_err();
        assert!(unknown.to_string().contains("unknown"));
        for malformed in [Value::from(("CSV",)), Value::from(("CSV", vec![0_u32]))] {
            let mut values = results(vec!["file:///a"], None);
            values.insert(
                "current_filter".into(),
                OwnedValue::try_from(malformed).unwrap(),
            );
            assert!(request.selection(values).is_err());
        }
    }
}
