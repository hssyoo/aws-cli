//! A paginated call, as `CLIOperationCaller._make_client_call` makes one
//! (`paginator.paginate(**parameters)`) and `_display_response`'s formatter
//! consumes the `PageIterator`: `JSONFormatter` prints its full result,
//! `TextFormatter` each page as it arrives, to stdout or into the pager,
//! and `OffFormatter` takes every page and prints nothing.
//!
//! Python's `KeyboardInterrupt` lands wherever the iteration is. Here the
//! pages are fetched as active work ([`Scope`]), so a Ctrl-C is recorded
//! rather than ending the process: the request in flight completes, as the
//! transport cannot cut one short, no further request starts
//! (`LazyTransport`), the page in flight is dropped, and the run ends as
//! `InterruptExceptionHandler` ends it, after the output already written.
use super::*;
use crate::admit::Paginated;
use cli_core::interrupt::{Scope, State};
use client::paginate::PageIterator;
use format::pages::{Pages, Stopped};
use model::value::Map;

/// The paginator's pages, each checked against a Ctrl-C recorded while it
/// was fetched.
struct Fetched<'c> {
    pages: PageIterator<'c>,
    interrupt: &'static State,
}

impl Pages for Fetched<'_> {
    type Error = CallError;
    fn next_page(&mut self) -> Option<Result<Value, CallError>> {
        let page = self.pages.next()?;
        if self.interrupt.is_requested() {
            return Some(Err(CallError::Interrupted));
        }
        Some(page)
    }
    fn result_keys(&self) -> &[String] {
        self.pages.result_keys()
    }
    fn non_aggregate_part(&self) -> &Map {
        self.pages.non_aggregate_part()
    }
    fn resume_token(&self) -> Option<&str> {
        self.pages.resume_token()
    }
}

/// The paginated call of `invocation`, printed as `ready.format` says.
pub(super) fn run(
    invocation: &ServiceInvocation,
    cx: &mut Context<'_>,
    ready: &Ready,
    client: &Client,
    paginated: &Paginated,
) -> Result<u8, CommandFailure> {
    let interrupt = cli_core::interrupt::process_state();
    let mut fetched = Fetched {
        pages: PageIterator::new(
            client,
            &invocation.admitted.operation,
            invocation.admitted.input.clone(),
            paginated.config.clone(),
            paginated.pagination.clone(),
        ),
        interrupt,
    };
    let mut scope = Scope::enter(interrupt);
    let full = match ready.format {
        OutputFormat::Text => {
            return match &ready.destination {
                Destination::Stdout => text_to_stdout(invocation, cx, &mut fetched, scope),
                Destination::Pager(argv) => {
                    text_to_pager(invocation, cx, ready, argv, &mut fetched, scope)
                }
            };
        }
        OutputFormat::Off => drain(&mut fetched).map(|()| None),
        OutputFormat::Json => fetched.pages.build_full_result().map(Some),
    };
    let full = if interrupt.is_requested() {
        Err(CallError::Interrupted)
    } else {
        full
    };
    scope.leave();
    match full {
        Ok(Some(full)) => invocation.print(cx, ready, &full),
        Ok(None) => Ok(0),
        Err(error) => invocation.call_failed(cx, error),
    }
}

/// `OffFormatter`: every page taken, none printed.
fn drain(fetched: &mut Fetched<'_>) -> Result<(), CallError> {
    while let Some(page) = fetched.next_page() {
        page?;
    }
    Ok(())
}

/// `TextFormatter` writing to stdout: each page as it is formatted, the
/// stream flushed at the end.
fn text_to_stdout(
    invocation: &ServiceInvocation,
    cx: &mut Context<'_>,
    fetched: &mut Fetched<'_>,
    mut scope: Scope<'_>,
) -> Result<u8, CommandFailure> {
    let stdout = &mut *cx.stdout;
    let written = format::pages::text(fetched, &mut |text| stdout.write_all(text.as_bytes()));
    // TextFormatter flushes the stream however the pages ended; only a
    // failure it raised before then is passed over.
    let flushed = stdout.flush();
    let written = written.and_then(|()| flushed.map_err(Stopped::Write));
    scope.leave();
    match written {
        Ok(()) => Ok(0),
        Err(stopped) => ended(invocation, cx, stopped),
    }
}

/// `TextFormatter` writing into the pager, which starts at the first page
/// with text (`LazyPager`) and is waited for (`process.communicate()`)
/// however the writing ended: an `OSError` writing to it ends the pages
/// without a failure (`get_pager_stream`), a missing pager program fails,
/// and anything else fails once the pager exits.
fn text_to_pager(
    invocation: &ServiceInvocation,
    cx: &mut Context<'_>,
    ready: &Ready,
    argv: &[String],
    fetched: &mut Fetched<'_>,
    mut scope: Scope<'_>,
) -> Result<u8, CommandFailure> {
    let mut pager = format::pager::Stream::new(argv, &ready.pager_env);
    let mut unstarted = None;
    let written = format::pages::text(fetched, &mut |text| match pager.write(text.as_bytes()) {
        Ok(true) => Ok(()),
        Ok(false) => Err(std::io::ErrorKind::BrokenPipe.into()),
        Err(error) => {
            unstarted = Some(error);
            Err(std::io::ErrorKind::NotFound.into())
        }
    });
    let interrupted_fetch = fetched.interrupt.is_requested();
    scope.leave();
    // The Ctrl-C that ended the pages is being handled while Python's
    // communicate() waits for the pager, so it does not cut that wait short.
    let handled = State::new();
    let wait = if interrupted_fetch {
        &handled
    } else {
        fetched.interrupt
    };
    let outcome = pager.finish(wait);
    if let Some(error) = unstarted {
        return Err(service_errors::pager_failure(&error));
    }
    if outcome == Outcome::Interrupted {
        return Ok(interrupted(cx));
    }
    match written {
        Ok(()) | Err(Stopped::Write(_)) => Ok(0),
        Err(stopped) => ended(invocation, cx, stopped),
    }
}

/// What the text formatter's early end comes to: the page's failure, the
/// formatter's exception, or the failed write (`GeneralExceptionHandler`).
fn ended(
    invocation: &ServiceInvocation,
    cx: &mut Context<'_>,
    stopped: Stopped<CallError>,
) -> Result<u8, CommandFailure> {
    match stopped {
        Stopped::Page(error) => invocation.call_failed(cx, error),
        Stopped::Format(error) => Err(general(error.message.into_bytes())),
        Stopped::Write(error) => Err(general(error.to_string().into_bytes())),
    }
}
