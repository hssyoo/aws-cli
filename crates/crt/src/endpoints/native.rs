use super::{Documents, ParamValue, Parameters, ResolveError, Resolved};
use crate::runtime::SdkutilsLibrary;
use crt_sys::{AwsByteCursor as Cursor, endpoints as c};
use std::marker::PhantomData;
use std::ptr::NonNull;

macro_rules! owner {
    ($name:ident,$target:ident,$release:ident) => {
        struct $name<'library>(NonNull<c::$target>, PhantomData<&'library SdkutilsLibrary>);
        impl<'library> $name<'library> {
            fn new(_library: &'library SdkutilsLibrary, pointer: *mut c::$target) -> Option<Self> {
                NonNull::new(pointer).map(|pointer| Self(pointer, PhantomData))
            }
        }
        impl Drop for $name<'_> {
            fn drop(&mut self) {
                // SAFETY: this owner holds exactly one counted reference from its constructor.
                unsafe {
                    c::$release(self.0.as_ptr());
                }
            }
        }
    };
}
owner!(Rules, Ruleset, aws_endpoints_ruleset_release);
owner!(Partitions, Partitions, aws_partitions_config_release);
owner!(Engine, Engine, aws_endpoints_rule_engine_release);
owner!(Context, Context, aws_endpoints_request_context_release);
owner!(
    ResultOwner,
    Resolved,
    aws_endpoints_resolved_endpoint_release
);

/// Evaluates `documents` for `parameters`, each passed to the engine by its
/// name, without reading process state. The result owns copies of the
/// CRT's bytes, so it outlives the engine.
pub fn resolve(
    documents: Documents<'_>,
    parameters: Parameters<'_>,
) -> Result<Resolved, ResolveError> {
    let library = SdkutilsLibrary::acquire();
    // SAFETY: default allocator is process-lifetime; constructors synchronously
    // consume the JSON cursors and return independent counted owners.
    let (_rules, _partitions, engine, context) = unsafe {
        let allocator = crt_sys::rust_crt_allocator();
        let rules = Rules::new(
            &library,
            c::aws_endpoints_ruleset_new_from_string(
                allocator,
                Cursor::from_bytes(documents.rules_json),
            ),
        )
        .ok_or(ResolveError::InvalidRuleset)?;
        let partitions = Partitions::new(
            &library,
            c::aws_partitions_config_new_from_string(
                allocator,
                Cursor::from_bytes(documents.partitions_json),
            ),
        )
        .ok_or(ResolveError::InvalidPartitions)?;
        let engine = Engine::new(
            &library,
            c::aws_endpoints_rule_engine_new(allocator, rules.0.as_ptr(), partitions.0.as_ptr()),
        )
        .ok_or(ResolveError::InvalidRuleset)?;
        let context = Context::new(&library, c::aws_endpoints_request_context_new(allocator))
            .ok_or(ResolveError::OutOfMemory)?;
        (rules, partitions, engine, context)
    };
    add_parameters(&context, parameters)?;
    let mut result = std::ptr::null_mut();
    // SAFETY: engine/context remain live through resolution; output is owned.
    if unsafe {
        c::aws_endpoints_rule_engine_resolve(engine.0.as_ptr(), context.0.as_ptr(), &mut result)
    } != 0
    {
        return Err(ResolveError::ResolveFailed);
    }
    let result = ResultOwner::new(&library, result).ok_or(ResolveError::ResolveFailed)?;
    copy_result(&result)
}
fn add_parameters(context: &Context<'_>, parameters: Parameters<'_>) -> Result<(), ResolveError> {
    for &(name, value) in parameters {
        match value {
            ParamValue::String(value) => add_string(context, name, value)?,
            ParamValue::Boolean(value) => add_boolean(context, name, value)?,
            ParamValue::StringArray(values) => add_string_array(context, name, values)?,
        }
    }
    Ok(())
}
fn copy_result(result: &ResultOwner<'_>) -> Result<Resolved, ResolveError> {
    let mut url = Cursor::from_bytes(&[]);
    let mut properties = url;
    // SAFETY: each borrowed cursor is copied before the counted result is freed.
    unsafe {
        if c::aws_endpoints_resolved_endpoint_get_type(result.0.as_ptr())
            == c::AWS_ENDPOINTS_RESOLVED_ERROR
        {
            if c::aws_endpoints_resolved_endpoint_get_error(result.0.as_ptr(), &mut url) != 0 {
                return Err(ResolveError::ResolveFailed);
            }
            return Ok(Resolved::RuleError(copy_cursor(url)));
        }
        if c::aws_endpoints_resolved_endpoint_get_url(result.0.as_ptr(), &mut url) != 0
            || c::aws_endpoints_resolved_endpoint_get_properties(result.0.as_ptr(), &mut properties)
                != 0
        {
            return Err(ResolveError::ResolveFailed);
        }
        Ok(Resolved::Endpoint {
            url: copy_cursor(url),
            properties: copy_cursor(properties),
        })
    }
}
fn add_string(context: &Context<'_>, name: &str, value: &str) -> Result<(), ResolveError> {
    // SAFETY: CRT copies parameter name/value bytes into the live context.
    let rc = unsafe {
        c::aws_endpoints_request_context_add_string(
            crt_sys::rust_crt_allocator(),
            context.0.as_ptr(),
            Cursor::from_bytes(name.as_bytes()),
            Cursor::from_bytes(value.as_bytes()),
        )
    };
    if rc != 0 {
        return Err(ResolveError::OutOfMemory);
    }
    Ok(())
}
fn add_boolean(context: &Context<'_>, name: &str, value: bool) -> Result<(), ResolveError> {
    // SAFETY: the live context copies the name and stores the scalar value.
    let rc = unsafe {
        c::aws_endpoints_request_context_add_boolean(
            crt_sys::rust_crt_allocator(),
            context.0.as_ptr(),
            Cursor::from_bytes(name.as_bytes()),
            value,
        )
    };
    if rc != 0 {
        return Err(ResolveError::OutOfMemory);
    }
    Ok(())
}
fn add_string_array(
    context: &Context<'_>,
    name: &str,
    values: &[&str],
) -> Result<(), ResolveError> {
    let cursors: Vec<Cursor> = values
        .iter()
        .map(|value| Cursor::from_bytes(value.as_bytes()))
        .collect();
    // SAFETY: the live context copies the name and every value, which
    // `cursors` borrows for the call.
    let rc = unsafe {
        c::aws_endpoints_request_context_add_string_array(
            crt_sys::rust_crt_allocator(),
            context.0.as_ptr(),
            Cursor::from_bytes(name.as_bytes()),
            cursors.as_ptr(),
            cursors.len(),
        )
    };
    if rc != 0 {
        return Err(ResolveError::OutOfMemory);
    }
    Ok(())
}
unsafe fn copy_cursor(cursor: Cursor) -> Vec<u8> {
    if cursor.len == 0 {
        return Vec::new();
    }
    // SAFETY: callers only pass cursors borrowing a still-live resolved endpoint.
    unsafe { std::slice::from_raw_parts(cursor.ptr, cursor.len).to_vec() }
}
