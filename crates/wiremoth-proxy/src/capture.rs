//! Streaming body capture: forwards every frame untouched while retaining a
//! bounded prefix for inspection.

use std::fmt::Display;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body, Frame, SizeHint};

use crate::events::CapturedBody;

/// Shared buffer a [`TeeBody`] appends to.
pub(crate) type BodyBuffer = Arc<Mutex<CapturedBody>>;

pub(crate) fn new_buffer() -> BodyBuffer {
    Arc::new(Mutex::new(CapturedBody::default()))
}

/// Lock helper that tolerates a poisoned mutex: capture state is diagnostic
/// data, not worth panicking the proxy over.
pub(crate) fn lock(buffer: &BodyBuffer) -> MutexGuard<'_, CapturedBody> {
    buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A copy of the buffer contents for handing to an event.
pub(crate) fn snapshot(buffer: &BodyBuffer) -> CapturedBody {
    lock(buffer).clone()
}

/// Why a [`TeeBody`] stopped being polled.
#[derive(Debug, Clone)]
pub(crate) enum EndReason {
    /// End of stream reached: the full body was observed.
    Complete,
    /// The wrapped body returned an error.
    Failed(String),
    /// Dropped before completion (client disconnect, cancellation, ...).
    Dropped,
}

/// Wraps a body, forwarding frames unchanged while copying the leading bytes
/// into a capture buffer, and reporting the end of the stream once.
pub(crate) struct TeeBody<B> {
    inner: B,
    buffer: BodyBuffer,
    limit: usize,
    on_end: Option<Box<dyn Fn(EndReason) + Send + Sync>>,
    ended: bool,
}

impl<B> TeeBody<B> {
    fn end(&mut self, reason: EndReason) {
        if self.ended {
            return;
        }
        self.ended = true;
        if let Some(on_end) = &self.on_end {
            on_end(reason);
        }
    }
}

impl<B> TeeBody<B>
where
    B: Body<Data = Bytes>,
{
    pub(crate) fn new(
        inner: B,
        limit: usize,
        buffer: BodyBuffer,
        on_end: Option<Box<dyn Fn(EndReason) + Send + Sync>>,
    ) -> Self {
        let mut tee = Self {
            inner,
            buffer,
            limit,
            on_end,
            ended: false,
        };
        // Zero-length bodies may never be polled at all; report them complete
        // up front so consumers do not see every empty body as aborted.
        if tee.inner.is_end_stream() {
            lock(&tee.buffer).mark_complete();
            tee.end(EndReason::Complete);
        }
        tee
    }
}

impl<B> Body for TeeBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Display,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(chunk) = frame.data_ref() {
                    lock(&this.buffer).append(chunk, this.limit);
                }
                // A body that advertises an exact length can become "ended"
                // as soon as its final frame is handed over; the writer may
                // then never poll again, so completion must be detected here
                // rather than waiting for a `None` frame.
                if this.inner.is_end_stream() {
                    lock(&this.buffer).mark_complete();
                    this.end(EndReason::Complete);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(err))) => {
                this.end(EndReason::Failed(err.to_string()));
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(None) => {
                lock(&this.buffer).mark_complete();
                this.end(EndReason::Complete);
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B> Drop for TeeBody<B> {
    fn drop(&mut self) {
        self.end(EndReason::Dropped);
    }
}
