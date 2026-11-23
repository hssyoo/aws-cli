use super::object::{self, Owned, PyResult};
use crate::ffi::PyObject;
use pyload::View;
use pyload::catalog::{self, GroupError, Layout};

pub(super) fn has_prefix(args: *mut PyObject) -> PyResult<Owned> {
    let args = object::args(args, c"O")?;
    let prefix = args[0].unicode()?;
    let view = object::view()?;
    let index = object::payload_result(view.first_name_at_least(prefix))?;
    let found = index < view.entry_count()
        && object::payload_result(view.entry_name_at(index))?.starts_with(prefix);
    object::boolean(found)
}

pub(super) fn list_prefix(args: *mut PyObject) -> PyResult<Owned> {
    list_names(args, false)
}

pub(super) fn list_dir(args: *mut PyObject) -> PyResult<Owned> {
    list_names(args, true)
}

fn list_names(args: *mut PyObject, children: bool) -> PyResult<Owned> {
    let args = object::args(args, c"O")?;
    let prefix = args[0].unicode()?;
    let view = object::view()?;
    let output = object::list()?;
    let first = object::payload_result(view.first_name_at_least(prefix))?;
    let mut previous: &[u8] = b"";
    for index in first..view.entry_count() {
        let name = object::payload_result(view.entry_name_at(index))?;
        if !name.starts_with(prefix) {
            break;
        }
        let Some(value) = listed_name(prefix, name, children) else {
            continue;
        };
        if children && value == previous {
            continue;
        }
        output.append(&object::string(value)?)?;
        previous = value;
    }
    Some(output)
}

fn listed_name<'a>(prefix: &[u8], name: &'a [u8], children: bool) -> Option<&'a [u8]> {
    if children {
        catalog::immediate_child(prefix, name)
    } else {
        Some(name)
    }
}

pub(super) fn list_services(args: *mut PyObject) -> PyResult<Owned> {
    let args = object::args(args, c"OO")?;
    let (prefix, type_name) = (args[0].unicode()?, args[1].unicode()?);
    list_groups(object::view()?, prefix, type_name, Layout::Services)
}

pub(super) fn list_api_versions(args: *mut PyObject) -> PyResult<Owned> {
    let args = object::args(args, c"OOO")?;
    let (prefix, service, type_name) = (args[0].unicode()?, args[1].unicode()?, args[2].unicode()?);
    let view = object::view()?;
    let prefix = lookup_parts(&[prefix, service, b"/"])?;
    list_groups(view, &prefix, type_name, Layout::Versions)
}

/// The walk's failure, kept until the Python exception can be raised.
enum Stop {
    Group(GroupError),
    Raised,
}

impl From<GroupError> for Stop {
    fn from(error: GroupError) -> Self {
        Self::Group(error)
    }
}

fn list_groups(view: View<'_>, prefix: &[u8], type_name: &[u8], layout: Layout) -> PyResult<Owned> {
    let output = object::list()?;
    let walked = catalog::for_each_model_group(view, prefix, type_name, layout, |group| {
        output
            .append(&object::string(group).ok_or(Stop::Raised)?)
            .ok_or(Stop::Raised)
    });
    match walked {
        Ok(()) => Some(output),
        Err(Stop::Raised) => None,
        Err(Stop::Group(GroupError::NameTooLong)) => {
            object::overflow(c"zippy: lookup name too long")
        }
        Err(Stop::Group(_)) => object::runtime_error(c"zippy: invalid payload"),
    }
}

pub(super) fn lookup_parts(parts: &[&[u8]]) -> PyResult<Vec<u8>> {
    object::bounded::<{ catalog::MODEL_GROUP_NAME_MAX }>(parts, c"zippy: lookup name too long")
}
