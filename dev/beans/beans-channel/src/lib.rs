use std::error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::usize;

use concurrent_queue::{ConcurrentQueue, PopError, PushError};
use event_listener::{Event, EventListener};
use futures_core::stream::Stream;


//------------------------------------------------------------------------------
//  Channel
//------------------------------------------------------------------------------
struct Channel<T>
{
    queue: ConcurrentQueue<T>,
    send_ops: Event,
    recv_ops: Event,
    stream_ops: Event,
    sender_count: AtomicUsize,
    receiver_count: AtomicUsize,
}

impl<T> Channel<T>
{
    //--------------------------------------------------------------------------
    //  close
    //--------------------------------------------------------------------------
    fn close(&self) -> bool
    {
        if self.queue.close()
        {
            self.send_ops.notify(usize::MAX);
            self.recv_ops.notify(usize::MAX);
            self.stream_ops.notify(usize::MAX);

            true
        }
        else
        {
            false
        }
    }
}


//------------------------------------------------------------------------------
//  bounded
//------------------------------------------------------------------------------
pub fn bounded<T>( cap: usize ) -> (Sender<T>, Receiver<T>)
{
    assert!(cap > 0, "capacity cannot be zero");

    let channel = Arc::new(Channel
    {
        queue: ConcurrentQueue::bounded(cap),
        send_ops: Event::new(),
        recv_ops: Event::new(),
        stream_ops: Event::new(),
        sender_count: AtomicUsize::new(1),
        receiver_count: AtomicUsize::new(1),
    });

    let s = Sender
    {
        channel: channel.clone(),
    };

    let r = Receiver
    {
        channel,
        listener: None,
    };

    (s, r)
}


//------------------------------------------------------------------------------
//  unbounded
//------------------------------------------------------------------------------
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>)
{
    let channel = Arc::new(Channel
    {
        queue: ConcurrentQueue::unbounded(),
        send_ops: Event::new(),
        recv_ops: Event::new(),
        stream_ops: Event::new(),
        sender_count: AtomicUsize::new(1),
        receiver_count: AtomicUsize::new(1),
    });

    let s = Sender
    {
        channel: channel.clone(),
    };

    let r = Receiver
    {
        channel,
        listener: None,
    };

    (s, r)
}


//------------------------------------------------------------------------------
//  Sender
//------------------------------------------------------------------------------
pub struct Sender<T>
{
    channel: Arc<Channel<T>>,
}

impl<T> Sender<T>
{
    //--------------------------------------------------------------------------
    //  try_send
    //--------------------------------------------------------------------------
    pub fn try_send( &self, msg: T ) -> Result<(), TrySendError<T>>
    {
        match self.channel.queue.push(msg)
        {
            Ok(()) =>
            {
                self.channel.recv_ops.notify(1);
                self.channel.stream_ops.notify(usize::MAX);
                Ok(())
            },
            Err(PushError::Full(msg)) => Err(TrySendError::Full(msg)),
            Err(PushError::Closed(msg)) => Err(TrySendError::Closed(msg)),
        }
    }

    //--------------------------------------------------------------------------
    //  send
    //--------------------------------------------------------------------------
    pub fn send( &self, msg: T ) -> Send<'_, T>
    {
        Send
        {
            sender: self,
            listener: None,
            msg: Some(msg),
        }
    }

    //--------------------------------------------------------------------------
    //  send_blocking
    //--------------------------------------------------------------------------
    pub fn send_blocking( &self, msg: T ) -> Result<(), SendError<T>>
    {
        self.send(msg).wait()
    }

    //--------------------------------------------------------------------------
    //  close
    //--------------------------------------------------------------------------
    pub fn close( &self ) -> bool
    {
        self.channel.close()
    }

    //--------------------------------------------------------------------------
    //  is_closed
    //--------------------------------------------------------------------------
    pub fn is_closed( &self ) -> bool
    {
        self.channel.queue.is_closed()
    }

    //--------------------------------------------------------------------------
    //  is_empty
    //--------------------------------------------------------------------------
    pub fn is_empty( &self ) -> bool
    {
        self.channel.queue.is_empty()
    }

    //--------------------------------------------------------------------------
    //  is_full
    //--------------------------------------------------------------------------
    pub fn is_full( &self ) -> bool
    {
        self.channel.queue.is_full()
    }

    //--------------------------------------------------------------------------
    //  len
    //--------------------------------------------------------------------------
    pub fn len( &self ) -> usize
    {
        self.channel.queue.len()
    }

    //--------------------------------------------------------------------------
    //  capacity
    //--------------------------------------------------------------------------
    pub fn capacity( &self ) -> Option<usize>
    {
        self.channel.queue.capacity()
    }

    //--------------------------------------------------------------------------
    //  receiver_count
    //--------------------------------------------------------------------------
    pub fn receiver_count( &self ) -> usize
    {
        self.channel.receiver_count.load(Ordering::SeqCst)
    }

    //--------------------------------------------------------------------------
    //  sender_count
    //--------------------------------------------------------------------------
    pub fn sender_count( &self ) -> usize
    {
        self.channel.sender_count.load(Ordering::SeqCst)
    }
}

impl<T> Drop for Sender<T>
{
    //--------------------------------------------------------------------------
    //  drop
    //--------------------------------------------------------------------------
    fn drop(&mut self)
    {
        if self.channel.sender_count.fetch_sub(1, Ordering::AcqRel) == 1
        {
            self.channel.close();
        }
    }
}

impl<T> fmt::Debug for Sender<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        write!(f, "Sender {{ .. }}")
    }
}

impl<T> Clone for Sender<T>
{
    //--------------------------------------------------------------------------
    //  clone
    //--------------------------------------------------------------------------
    fn clone( &self ) -> Sender<T>
    {
        let count = self.channel.sender_count.fetch_add(1, Ordering::Relaxed);

        if count > usize::MAX / 2
        {
            process::abort();
        }

        Sender
        {
            channel: self.channel.clone(),
        }
    }
}


//------------------------------------------------------------------------------
//  Receiver
//------------------------------------------------------------------------------
pub struct Receiver<T>
{
    channel: Arc<Channel<T>>,
    listener: Option<EventListener>,
}

impl<T> Receiver<T>
{
    //--------------------------------------------------------------------------
    //  try_recv
    //--------------------------------------------------------------------------
    pub fn try_recv( &self ) -> Result<T, TryRecvError>
    {
        match self.channel.queue.pop()
        {
            Ok(msg) =>
            {
                self.channel.send_ops.notify(1);
                Ok(msg)
            },
            Err(PopError::Empty) => Err(TryRecvError::Empty),
            Err(PopError::Closed) => Err(TryRecvError::Closed),
        }
    }

    //--------------------------------------------------------------------------
    //  recv
    //--------------------------------------------------------------------------
    pub fn recv( &self ) -> Recv<'_, T>
    {
        Recv
        {
            receiver: self,
            listener: None,
        }
    }

    //--------------------------------------------------------------------------
    //  recv_blocking
    //--------------------------------------------------------------------------
    pub fn recv_blocking( &self ) -> Result<T, RecvError>
    {
        self.recv().wait()
    }

    //--------------------------------------------------------------------------
    //  close
    //--------------------------------------------------------------------------
    pub fn close( &self ) -> bool
    {
        self.channel.close()
    }

    //--------------------------------------------------------------------------
    //  is_close
    //--------------------------------------------------------------------------
    pub fn is_closed( &self ) -> bool
    {
        self.channel.queue.is_closed()
    }

    //--------------------------------------------------------------------------
    //  is_empty
    //--------------------------------------------------------------------------
    pub fn is_empty( &self ) -> bool
    {
        self.channel.queue.is_empty()
    }

    //--------------------------------------------------------------------------
    //  is_full
    //--------------------------------------------------------------------------
    pub fn is_full( &self ) -> bool
    {
        self.channel.queue.is_full()
    }

    //--------------------------------------------------------------------------
    //  len
    //--------------------------------------------------------------------------
    pub fn len( &self ) -> usize
    {
        self.channel.queue.len()
    }

    //--------------------------------------------------------------------------
    //  capacity
    //--------------------------------------------------------------------------
    pub fn capacity( &self ) -> Option<usize>
    {
        self.channel.queue.capacity()
    }

    //--------------------------------------------------------------------------
    //  receiver_count
    //--------------------------------------------------------------------------
    pub fn receiver_count( &self ) -> usize
    {
        self.channel.receiver_count.load(Ordering::SeqCst)
    }

    //--------------------------------------------------------------------------
    //  sender_count
    //--------------------------------------------------------------------------
    pub fn sender_count( &self ) -> usize
    {
        self.channel.sender_count.load(Ordering::SeqCst)
    }
}

impl<T> Drop for Receiver<T>
{
    //--------------------------------------------------------------------------
    //  drop
    //--------------------------------------------------------------------------
    fn drop( &mut self )
    {
        if self.channel.receiver_count.fetch_sub(1, Ordering::AcqRel) == 1
        {
            self.channel.close();
        }
    }
}

impl<T> fmt::Debug for Receiver<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        write!(f, "Receiver {{ .. }}")
    }
}

impl<T> Clone for Receiver<T>
{
    //--------------------------------------------------------------------------
    //  clone
    //--------------------------------------------------------------------------
    fn clone( &self ) -> Receiver<T>
    {
        let count = self.channel.receiver_count.fetch_add(1, Ordering::Relaxed);

        if count > usize::MAX / 2
        {
            process::abort();
        }

        Receiver
        {
            channel: self.channel.clone(),
            listener: None,
        }
    }
}

impl<T> Stream for Receiver<T>
{
    type Item = T;

    //--------------------------------------------------------------------------
    //  poll_next
    //--------------------------------------------------------------------------
    fn poll_next( mut self: Pin<&mut Self>, cx: &mut Context<'_> )
        -> Poll<Option<Self::Item>>
    {
        loop
        {
            if let Some(listener) = self.listener.as_mut()
            {
                futures_core::ready!(Pin::new(listener).poll(cx));
                self.listener = None;
            }

            loop
            {
                match self.try_recv()
                {
                    Ok(msg) =>
                    {
                        self.listener = None;
                        return Poll::Ready(Some(msg));
                    },
                    Err(TryRecvError::Closed) =>
                    {
                        self.listener = None;
                        return Poll::Ready(None);
                    },
                    Err(TryRecvError::Empty) => {},
                }

                match self.listener.as_mut()
                {
                    None =>
                    {
                        self.listener = Some(self.channel.stream_ops.listen());
                    },
                    Some(_) => { break; },
                }
            }
        }
    }
}

impl<T> futures_core::stream::FusedStream for Receiver<T>
{
    //--------------------------------------------------------------------------
    //  is_terminated
    //--------------------------------------------------------------------------
    fn is_terminated( &self ) -> bool
    {
        self.channel.queue.is_closed() && self.channel.queue.is_empty()
    }
}


//------------------------------------------------------------------------------
//  SendError
//------------------------------------------------------------------------------
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct SendError<T>(pub T);

impl<T> SendError<T>
{
    //--------------------------------------------------------------------------
    //  into_inner
    //--------------------------------------------------------------------------
    pub fn into_inner( self ) -> T
    {
        self.0
    }
}

impl<T> error::Error for SendError<T> {}

impl<T> fmt::Debug for SendError<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        write!(f, "SendError(..)")
    }
}

impl<T> fmt::Display for SendError<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        write!(f, "sending into a closed channel")
    }
}


//------------------------------------------------------------------------------
//  TrySendError
//------------------------------------------------------------------------------
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum TrySendError<T>
{
    Full(T),
    Closed(T),
}

impl<T> TrySendError<T>
{
    //--------------------------------------------------------------------------
    //  into_inner
    //--------------------------------------------------------------------------
    pub fn into_inner( self ) -> T
    {
        match self
        {
            TrySendError::Full(t) => t,
            TrySendError::Closed(t) => t,
        }
    }

    //--------------------------------------------------------------------------
    //  is_full
    //--------------------------------------------------------------------------
    pub fn is_full( &self ) -> bool
    {
        match self
        {
            TrySendError::Full(_) => true,
            TrySendError::Closed(_) => false,
        }
    }

    //--------------------------------------------------------------------------
    //  is_closed
    //--------------------------------------------------------------------------
    pub fn is_closed( &self ) -> bool
    {
        match self
        {
            TrySendError::Full(_) => false,
            TrySendError::Closed(_) => true,
        }
    }
}

impl<T> error::Error for TrySendError<T> {}

impl<T> fmt::Debug for TrySendError<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        match *self
        {
            TrySendError::Full(..) => write!(f, "Full(..)"),
            TrySendError::Closed(..) => write!(f, "Closed(..)"),
        }
    }
}

impl<T> fmt::Display for TrySendError<T>
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        match *self
        {
            TrySendError::Full(..) => write!(f, "sending into a full channel"),
            TrySendError::Closed(..) => write!(f, "sending into a closed channel"),
        }
    }
}


//------------------------------------------------------------------------------
//  RecvError
//------------------------------------------------------------------------------
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct RecvError;

impl error::Error for RecvError {}

impl fmt::Display for RecvError
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        write!(f, "receiving from an empty and closed channel")
    }
}


//------------------------------------------------------------------------------
//  TryRecvError
//------------------------------------------------------------------------------
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum TryRecvError
{
    Empty,
    Closed,
}

impl TryRecvError
{
    //--------------------------------------------------------------------------
    //  is_empty
    //--------------------------------------------------------------------------
    pub fn is_empty( &self ) -> bool
    {
        match self
        {
            TryRecvError::Empty => true,
            TryRecvError::Closed => false,
        }
    }

    //--------------------------------------------------------------------------
    //  is_closed
    //--------------------------------------------------------------------------
    pub fn is_closed( &self ) -> bool
    {
        match self
        {
            TryRecvError::Empty => false,
            TryRecvError::Closed => true,
        }
    }
}

impl error::Error for TryRecvError {}

impl fmt::Display for TryRecvError
{
    //--------------------------------------------------------------------------
    //  fmt
    //--------------------------------------------------------------------------
    fn fmt( &self, f: &mut fmt::Formatter<'_> ) -> fmt::Result
    {
        match *self
        {
            TryRecvError::Empty => write!(f, "receiving from an empty channel"),
            TryRecvError::Closed => write!(f, "receiving from an empty and closed channel"),
        }
    }
}


//------------------------------------------------------------------------------
//  Send
//------------------------------------------------------------------------------
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Send<'a, T>
{
    sender: &'a Sender<T>,
    listener: Option<EventListener>,
    msg: Option<T>,
}

impl<'a, T> Send<'a, T>
{
    //--------------------------------------------------------------------------
    //  run_with_strategy
    //--------------------------------------------------------------------------
    fn run_with_strategy<S: Strategy>(
        &mut self,
        cx: &mut S::Context,
    ) -> Poll<Result<(), SendError<T>>>
    {
        loop
        {
            let msg = self.msg.take().unwrap();
            match self.sender.try_send(msg)
            {
                Ok(()) =>
                {
                    match self.sender.channel.queue.capacity()
                    {
                        Some(1) => {},
                        Some(_) | None => self.sender.channel.send_ops.notify(1),
                    }
                    return Poll::Ready(Ok(()));
                },
                Err(TrySendError::Closed(msg)) => return Poll::Ready(Err(SendError(msg))),
                Err(TrySendError::Full(m)) => self.msg = Some(m),
            }

            match self.listener.take()
            {
                None =>
                {
                    self.listener = Some(self.sender.channel.send_ops.listen());
                },
                Some(l) =>
                {
                    if let Err(l) = S::poll(l, cx)
                    {
                        self.listener = Some(l);
                        return Poll::Pending;
                    }
                },
            }
        }
    }

    //--------------------------------------------------------------------------
    //  wait
    //--------------------------------------------------------------------------
    fn wait( mut self ) -> Result<(), SendError<T>>
    {
        match self.run_with_strategy::<Blocking>(&mut ())
        {
            Poll::Ready(res) => res,
            Poll::Pending => unreachable!(),
        }
    }
}

impl<'a, T> Unpin for Send<'a, T> {}

impl<'a, T> Future for Send<'a, T>
{
    type Output = Result<(), SendError<T>>;

    //--------------------------------------------------------------------------
    //  poll
    //--------------------------------------------------------------------------
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Self::Output>
    {
        self.run_with_strategy::<NonBlocking<'_>>(cx)
    }
}


//------------------------------------------------------------------------------
//  Recv
//------------------------------------------------------------------------------
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct Recv<'a, T>
{
    receiver: &'a Receiver<T>,
    listener: Option<EventListener>,
}

impl<'a, T> Unpin for Recv<'a, T> {}

impl<'a, T> Recv<'a, T>
{
    //--------------------------------------------------------------------------
    //  poll
    //--------------------------------------------------------------------------
    fn run_with_strategy<S: Strategy>(
        &mut self,
        cx: &mut S::Context,
    ) -> Poll<Result<T, RecvError>>
    {
        loop
        {
            match self.receiver.try_recv()
            {
                Ok(msg) =>
                {
                    match self.receiver.channel.queue.capacity()
                    {
                        Some(1) => {},
                        Some(_) | None => self.receiver.channel.recv_ops.notify(1),
                    }
                    return Poll::Ready(Ok(msg));
                },
                Err(TryRecvError::Closed) => return Poll::Ready(Err(RecvError)),
                Err(TryRecvError::Empty) => {},
            }

            match self.listener.take()
            {
                None =>
                {
                    self.listener = Some(self.receiver.channel.recv_ops.listen());
                },
                Some(l) =>
                {
                    if let Err(l) = S::poll(l, cx)
                    {
                        self.listener = Some(l);
                        return Poll::Pending;
                    }
                },
            }
        }
    }

    //--------------------------------------------------------------------------
    //  wait
    //--------------------------------------------------------------------------
    fn wait( mut self ) -> Result<T, RecvError>
    {
        match self.run_with_strategy::<Blocking>(&mut ())
        {
            Poll::Ready(res) => res,
            Poll::Pending => unreachable!(),
        }
    }
}

impl<'a, T> Future for Recv<'a, T>
{
    type Output = Result<T, RecvError>;

    //--------------------------------------------------------------------------
    //  poll
    //--------------------------------------------------------------------------
    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Self::Output>
    {
        self.run_with_strategy::<NonBlocking<'_>>(cx)
    }
}


//------------------------------------------------------------------------------
//  Strategy
//------------------------------------------------------------------------------
trait Strategy
{
    type Context;

    fn poll(evl: EventListener, cx: &mut Self::Context) -> Result<(), EventListener>;
}


//------------------------------------------------------------------------------
//  NonBlocking
//------------------------------------------------------------------------------
struct NonBlocking<'a>(&'a mut ());

impl<'a> Strategy for NonBlocking<'a>
{
    type Context = Context<'a>;

    //--------------------------------------------------------------------------
    //  poll
    //--------------------------------------------------------------------------
    fn poll(
        mut evl: EventListener,
        cx: &mut Context<'a>
    ) -> Result<(), EventListener>
    {
        match Pin::new(&mut evl).poll(cx)
        {
            Poll::Ready(()) => Ok(()),
            Poll::Pending => Err(evl),
        }
    }
}


//------------------------------------------------------------------------------
//  Blocking
//------------------------------------------------------------------------------
struct Blocking;

impl Strategy for Blocking
{
    type Context = ();

    //--------------------------------------------------------------------------
    //  poll
    //--------------------------------------------------------------------------
    fn poll(evl: EventListener, _cx: &mut ()) -> Result<(), EventListener>
    {
        evl.wait();
        Ok(())
    }
}
