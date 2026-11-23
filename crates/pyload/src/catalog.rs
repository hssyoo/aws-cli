//! Byte-oriented payload catalog operations.
#![forbid(unsafe_code)]

pub fn prefix_upper_bound<'a>(buffer: &'a mut [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if prefix.len() > buffer.len() {
        return None;
    }
    let index = prefix.iter().rposition(|&byte| byte != 0xff)?;
    buffer[..index].copy_from_slice(&prefix[..index]);
    buffer[index] = prefix[index] + 1;
    Some(&buffer[..=index])
}
pub fn immediate_child<'a>(prefix: &[u8], name: &'a [u8]) -> Option<&'a [u8]> {
    let suffix = name.strip_prefix(prefix)?;
    let end = suffix
        .iter()
        .position(|&byte| byte == b'/')
        .unwrap_or(suffix.len());
    (end > 0).then_some(&suffix[..end])
}
pub fn is_model_file(file_name: &[u8], type_name: &[u8]) -> bool {
    file_name.strip_prefix(type_name) == Some(b".json")
}

/// The longest group lookup name, `<prefix><group>/`, a group walk builds.
pub const MODEL_GROUP_NAME_MAX: usize = 4096;
/// Which botocore data layout a group walk reads below its prefix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    /// `<service>/<version>/<type>.json`: the groups are services.
    Services,
    /// `<version>/<type>.json`: the groups are one service's API versions.
    Versions,
}
/// Why a group walk stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupError {
    /// The payload could not be read.
    Payload(crate::Error),
    /// The payload's names are not sorted, so a group's range is empty.
    Unsorted,
    /// A group's lookup name is longer than [`MODEL_GROUP_NAME_MAX`].
    NameTooLong,
}
impl core::fmt::Display for GroupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Payload(error) => error.fmt(f),
            Self::Unsorted => f.write_str("payload names are not sorted"),
            Self::NameTooLong => f.write_str("lookup name too long"),
        }
    }
}
impl std::error::Error for GroupError {}
impl From<crate::Error> for GroupError {
    fn from(error: crate::Error) -> Self {
        Self::Payload(error)
    }
}
/// Calls `found` with each group directly below `prefix` that holds a
/// `<type_name>.json` model, in name order, reading names only.
///
/// This is the payload form of botocore's `Loader.list_available_services`
/// ([`Layout::Services`]) and `Loader.list_api_versions` ([`Layout::Versions`]):
/// a group counts once one of its entries is that model file.
pub fn for_each_model_group<'a, E: From<GroupError>>(
    view: crate::View<'a>,
    prefix: &[u8],
    type_name: &[u8],
    layout: Layout,
    mut found: impl FnMut(&'a [u8]) -> Result<(), E>,
) -> Result<(), E> {
    let entries = view.entry_count();
    let mut index = view.first_name_at_least(prefix).map_err(GroupError::from)?;
    while index < entries {
        let name = view.entry_name_at(index).map_err(GroupError::from)?;
        let Some(relative) = name.strip_prefix(prefix) else {
            break;
        };
        let Some(group) = group_name(relative) else {
            index += 1;
            continue;
        };
        let mut buffer = [0; MODEL_GROUP_NAME_MAX];
        let group_prefix = joined(&mut buffer, &[prefix, group, b"/"])?;
        let limit = prefix_end(view, group_prefix)?;
        if group_has_model(view, index..limit, group_prefix, type_name, layout)? {
            found(group)?;
        }
        index = limit;
    }
    Ok(())
}
fn group_name(relative: &[u8]) -> Option<&[u8]> {
    let end = relative.iter().position(|byte| *byte == b'/')?;
    (end != 0).then_some(&relative[..end])
}
fn group_has_model(
    view: crate::View<'_>,
    range: core::ops::Range<usize>,
    prefix: &[u8],
    type_name: &[u8],
    layout: Layout,
) -> Result<bool, GroupError> {
    if range.end <= range.start {
        return Err(GroupError::Unsorted);
    }
    for index in range {
        let candidate = view.entry_name_at(index)?;
        let relative = candidate.strip_prefix(prefix).ok_or(GroupError::Unsorted)?;
        let file = match layout {
            Layout::Versions => Some(relative),
            Layout::Services => group_name(relative).map(|version| &relative[version.len() + 1..]),
        };
        if file.is_some_and(|file| is_model_file(file, type_name)) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn prefix_end(view: crate::View<'_>, prefix: &[u8]) -> Result<usize, GroupError> {
    let mut buffer = [0; MODEL_GROUP_NAME_MAX];
    match prefix_upper_bound(&mut buffer, prefix) {
        Some(upper) => Ok(view.first_name_at_least(upper)?),
        None => Ok(view.entry_count()),
    }
}
fn joined<'b>(buffer: &'b mut [u8], parts: &[&[u8]]) -> Result<&'b [u8], GroupError> {
    let mut length = 0;
    for part in parts {
        let end = length + part.len();
        buffer
            .get_mut(length..end)
            .ok_or(GroupError::NameTooLong)?
            .copy_from_slice(part);
        length = end;
    }
    Ok(&buffer[..length])
}

#[cfg(test)]
mod tests;
