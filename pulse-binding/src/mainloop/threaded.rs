// Copyright 2017 Lyndon Brown
//
// This file is part of the PulseAudio Rust language binding.
//
// This library is free software; you can redistribute it and/or modify it under the terms of the
// GNU Lesser General Public License as published by the Free Software Foundation; either version
// 2.1 of the License, or (at your option) any later version.
//
// This library is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without
// even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
// Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License along with this library;
// if not, see <http://www.gnu.org/licenses/>.

//! A variation of the standard main loop implementation, using a background thread.
//!
//! # Overview
//!
//! The threaded main loop implementation is a special version of the standard main loop
//! implementation. For the basic design, see the standard main loop documentation
//! ([`mainloop::standard`]).
//!
//! The added feature in the threaded main loop is that it spawns a new thread that runs the real
//! main loop in the background. This allows a synchronous application to use the asynchronous API
//! without risking stalling the PulseAudio library. A few synchronization primitives are available
//! to access the objects attached to the event loop safely.
//!
//! # Creation
//!
//! A [`Mainloop`] object is created using [`Mainloop::new`]. This will only allocate the required
//! structures though, so to use it the thread must also be started. This is done through
//! [`Mainloop::start`], after which you can start using the main loop.
//!
//! # Destruction
//!
//! When the PulseAudio connection has been terminated, the thread must be stopped and the
//! resources freed. Stopping the thread is done using [`Mainloop::stop`], which must be called
//! without the lock (see below) held. When that function returns, the thread is stopped and the
//! [`Mainloop`] object can be destroyed.
//!
//! Destruction of the [`Mainloop`] object is done automatically when the object falls out of scope.
//! (Rust’s `Drop` trait has been implemented and takes care of it).
//!
//! # Locking
//!
//! Since the PulseAudio API doesn’t allow concurrent accesses to objects, a locking scheme must be
//! used to guarantee safe usage. The threaded main loop API provides such a scheme through the
//! functions [`Mainloop::lock`] and [`Mainloop::unlock`].
//!
//! The lock is recursive, so it’s safe to use it multiple times from the same thread. Just make
//! sure you call [`Mainloop::unlock`] the same number of times you called [`Mainloop::lock`].
//!
//! The lock needs to be held whenever you call any PulseAudio function that uses an object
//! associated with this main loop. Make sure you do not hold on to the lock more than necessary
//! though, as the threaded main loop stops while the lock is held.
//!
//! Example:
//!
//! ```rust,ignore
//! extern crate libpulse_binding as pulse;
//!
//! use std::rc::Rc;
//! use std::cell::RefCell;
//! use pulse::mainloop::threaded::Mainloop;
//! use pulse::stream:{Stream, State};
//!
//! fn check_stream(m: Rc<RefCell<Mainloop>>, s: Rc<RefCell<Stream>>) {
//!     m.borrow_mut().lock();
//!
//!     let state = s.borrow().get_state();
//!
//!     m.borrow_mut().unlock();
//!
//!     match state {
//!         State::Ready => { printf!("Stream is ready!"); },
//!         _ => { printf!("Stream is not ready!"); },
//!     }
//! }
//! ```
//!
//! # Callbacks
//!
//! Callbacks in PulseAudio are asynchronous, so they require extra care when using them together
//! with a threaded main loop.
//!
//! The easiest way to turn the callback based operations into synchronous ones, is to simply wait
//! for the callback to be called and continue from there. This is the approach chosen in
//! PulseAudio’s threaded API.
//!
//! ## Basic callbacks
//!
//! For the basic case, where all that is required is to wait for the callback to be invoked, the
//! code should look something like this:
//!
//! Example:
//!
//! ```rust,ignore
//! extern crate libpulse_binding as pulse;
//!
//! use std::rc::Rc;
//! use std::cell::RefCell;
//! use pulse::mainloop::threaded::Mainloop;
//! use pulse::operation::State;
//! use pulse::stream:Stream;
//!
//! fn drain_stream(m: Rc<RefCell<Mainloop>>, s: Rc<RefCell<Stream>>) {
//!     m.borrow_mut().lock();
//!
//!     // Drain
//!     let o = {
//!         let ml_ref = Rc::clone(&m);
//!         s.borrow_mut().drain(Some(Box::new(move |_success: bool| {
//!             unsafe { (*ml_ref.as_ptr()).signal(false); }
//!         })))
//!     };
//!     while o.get_state() != pulse::operation::State::Done {
//!         m.borrow_mut().wait();
//!     }
//!
//!     m.borrow_mut().unlock();
//! }
//! ```
//!
//! The function `drain_stream` will wait for the callback to be called using [`Mainloop::wait`].
//!
//! If your application is multi-threaded, then this waiting must be done inside a while loop. The
//! reason for this is that multiple threads might be using [`Mainloop::wait`] at the same time.
//! Each thread must therefore verify that it was its callback that was invoked. Also the underlying
//! OS synchronization primitives are usually not free of spurious wake-ups, so a [`Mainloop::wait`]
//! must be called within a loop even if you have only one thread waiting.
//!
//! The callback `my_drain_callback` indicates to the main function that it has been called using
//! [`Mainloop::signal`].
//!
//! As you can see, [`Mainloop::wait`] may only be called with the lock held. The same thing is true
//! for [`Mainloop::signal`], but as the lock is held before the callback is invoked, you do not
//! have to deal with that.
//!
//! The functions will not dead lock because the wait function will release the lock before waiting
//! and then regrab it once it has been signalled. For those of you familiar with threads, the
//! behaviour is that of a condition variable.
//!
//! ## Data callbacks
//!
//! For many callbacks, simply knowing that they have been called is insufficient. The callback also
//! receives some data that is desired. To access this data safely, we must extend our example a
//! bit:
//!
//! ```rust,ignore
//! extern crate libpulse_binding as pulse;
//!
//! use std::rc::Rc;
//! use std::cell::RefCell;
//! use pulse::mainloop::threaded::Mainloop;
//! use pulse::stream:Stream;
//!
//! struct DrainCbData(Option<&mut bool>);
//!
//! fn drain_stream(m: Rc<RefCell<Mainloop>>, s: Rc<RefCell<Stream>>) {
//!     m.borrow_mut().lock();
//!
//!     let mut data = DrainCbData(None);
//!
//!     // Drain
//!     let o = {
//!         let ml_ref = Rc::clone(&m);
//!         s.borrow_mut().drain(Some(Box::new(move |success: bool| {
//!             data.0 = Some(&mut success);
//!             unsafe { (*ml_ref.as_ptr()).signal(true); }
//!         })))
//!     };
//!     while o.get_state() != pulse::operation::State::Done {
//!         m.borrow_mut().wait();
//!     }
//!
//!     assert!(!data.0.is_none());
//!     let success = *(data.0.take());
//!     m.borrow_mut().accept(); // Allow callback to continue now
//!
//!     match success {
//!         0 => { println!("Bitter defeat..."); },
//!         _ => { println!("Success!"); },
//!     }
//!
//!     m.borrow_mut().unlock();
//! }
//! ```
//!
//! The example is a bit silly as it would have been more simple to just copy the contents of
//! `success`, but for larger data structures this can be wasteful.
//!
//! The difference here compared to the basic callback is the value `true` passed to
//! [`Mainloop::signal`] and the call to [`Mainloop::accept`]. What will happen is that
//! [`Mainloop::signal`] will signal the main function and then wait. The main function is then free
//! to use the data in the callback until [`Mainloop::accept`] is called, which will allow the
//! callback to continue.
//!
//! Note that [`Mainloop::accept`] must be called some time between exiting the while loop and
//! unlocking the main loop! Failure to do so will result in a race condition. I.e. it is not okay
//! to release the lock and regrab it before calling [`Mainloop::accept`].
//!
//! ## Asynchronous callbacks
//!
//! PulseAudio also has callbacks that are completely asynchronous, meaning that they can be called
//! at any time. The threaded main loop API provides the locking mechanism to handle concurrent
//! accesses, but nothing else. Applications will have to handle communication from the callback to
//! the main program through their own mechanisms.
//!
//! The callbacks that are completely asynchronous are:
//!
//! * State callbacks for contexts, streams, etc.
//! * Subscription notifications.
//!
//! # Example
//!
//! An example program using the threaded mainloop:
//!
//! ```rust
//! extern crate libpulse_binding as pulse;
//!
//! use std::rc::Rc;
//! use std::cell::RefCell;
//! use std::ops::Deref;
//! use pulse::mainloop::threaded::Mainloop;
//! use pulse::context::Context;
//! use pulse::stream::Stream;
//! use pulse::proplist::Proplist;
//! use pulse::mainloop::api::Mainloop as MainloopTrait; //Needs to be in scope
//!
//! fn main() {
//!     let spec = pulse::sample::Spec {
//!         format: pulse::sample::SAMPLE_S16NE,
//!         channels: 2,
//!         rate: 44100,
//!     };
//!     assert!(spec.is_valid());
//!
//!     let mut proplist = Proplist::new().unwrap();
//!     proplist.set_str(pulse::proplist::properties::APPLICATION_NAME, "FooApp")
//!         .unwrap();
//!
//!     let mut mainloop = Rc::new(RefCell::new(Mainloop::new()
//!         .expect("Failed to create mainloop")));
//!
//!     let mut context = Rc::new(RefCell::new(Context::new_with_proplist(
//!         mainloop.borrow().deref(),
//!         "FooAppContext",
//!         &proplist
//!         ).expect("Failed to create new context")));
//!
//!     // Context state change callback
//!     {
//!         let ml_ref = Rc::clone(&mainloop);
//!         let context_ref = Rc::clone(&context);
//!         context.borrow_mut().set_state_callback(Some(Box::new(move || {
//!             let state = unsafe { (*context_ref.as_ptr()).get_state() };
//!             match state {
//!                 pulse::context::State::Ready |
//!                 pulse::context::State::Failed |
//!                 pulse::context::State::Terminated => {
//!                     unsafe { (*ml_ref.as_ptr()).signal(false); }
//!                 },
//!                 _ => {},
//!             }
//!         })));
//!     }
//!
//!     context.borrow_mut().connect(None, pulse::context::flags::NOFLAGS, None)
//!         .expect("Failed to connect context");
//!
//!     mainloop.borrow_mut().lock();
//!     mainloop.borrow_mut().start().expect("Failed to start mainloop");
//!
//!     // Wait for context to be ready
//!     loop {
//!         match context.borrow().get_state() {
//!             pulse::context::State::Ready => { break; },
//!             pulse::context::State::Failed |
//!             pulse::context::State::Terminated => {
//!                 eprintln!("context state failed/terminated, quitting...");
//!                 mainloop.borrow_mut().unlock();
//!                 mainloop.borrow_mut().stop();
//!                 return;
//!             },
//!             _ => { mainloop.borrow_mut().wait(); },
//!         }
//!     }
//!     context.borrow_mut().set_state_callback(None);
//!
//!     let mut stream = Rc::new(RefCell::new(Stream::new(
//!         &mut context.borrow_mut(),
//!         "Music",
//!         &spec,
//!         None
//!         ).expect("Failed to create new stream")));
//!
//!     // Stream state change callback
//!     {
//!         let ml_ref = Rc::clone(&mainloop);
//!         let stream_ref = Rc::clone(&stream);
//!         stream.borrow_mut().set_state_callback(Some(Box::new(move || {
//!             let state = unsafe { (*stream_ref.as_ptr()).get_state() };
//!             match state {
//!                 pulse::stream::State::Ready |
//!                 pulse::stream::State::Failed |
//!                 pulse::stream::State::Terminated => {
//!                     unsafe { (*ml_ref.as_ptr()).signal(false); }
//!                 },
//!                 _ => {},
//!             }
//!         })));
//!     }
//!
//!     stream.borrow_mut().connect_playback(None, None, pulse::stream::flags::START_CORKED,
//!         None, None).expect("Failed to connect playback");
//!
//!     // Wait for stream to be ready
//!     loop {
//!         match stream.borrow().get_state() {
//!             pulse::stream::State::Ready => { break; },
//!             pulse::stream::State::Failed |
//!             pulse::stream::State::Terminated => {
//!                 eprintln!("stream state failed/terminated, quitting...");
//!                 mainloop.borrow_mut().unlock();
//!                 mainloop.borrow_mut().stop();
//!                 return;
//!             },
//!             _ => { mainloop.borrow_mut().wait(); },
//!         }
//!     }
//!     stream.borrow_mut().set_state_callback(None);
//!
//!     mainloop.borrow_mut().unlock();
//!
//!     // Our main loop
//! #   let mut count = 0; // For automatic unit tests, we’ll spin a few times
//!     loop {
//!         mainloop.borrow_mut().lock();
//!
//!         // Write some data with stream.write()
//!
//!         if stream.borrow().is_corked().unwrap() {
//!             stream.borrow_mut().uncork(None);
//!         }
//!
//!         // Drain
//!         let o = {
//!             let ml_ref = Rc::clone(&mainloop);
//!             stream.borrow_mut().drain(Some(Box::new(move |_success: bool| {
//!                 unsafe { (*ml_ref.as_ptr()).signal(false); }
//!             })))
//!         };
//!         while o.get_state() != pulse::operation::State::Done {
//!             mainloop.borrow_mut().wait();
//!         }
//!
//!         mainloop.borrow_mut().unlock();
//!
//!         // If done writing data, call `mainloop.borrow_mut().stop()` (with lock released), then
//!         // break!
//! # 
//! #       // Hack: Stop test getting stuck in infinite loop!
//! #       count += 1;
//! #       if count == 3 {
//! #           mainloop.borrow_mut().stop();
//! #           break;
//! #       }
//!     }
//!
//!     // Clean shutdown
//!     mainloop.borrow_mut().lock();
//!     stream.borrow_mut().disconnect().unwrap();
//!     mainloop.borrow_mut().unlock();
//! }
//! ```
//!
//! [`mainloop::standard`]: ../standard/index.html
//! [`Mainloop`]: struct.Mainloop.html
//! [`Mainloop::new`]: struct.Mainloop.html#method.new
//! [`Mainloop::start`]: struct.Mainloop.html#method.start
//! [`Mainloop::stop`]: struct.Mainloop.html#method.stop
//! [`Mainloop::lock`]: struct.Mainloop.html#method.lock
//! [`Mainloop::unlock`]: struct.Mainloop.html#method.unlock
//! [`Mainloop::wait`]: struct.Mainloop.html#method.wait
//! [`Mainloop::signal`]: struct.Mainloop.html#method.signal
//! [`Mainloop::accept`]: struct.Mainloop.html#method.accept

use std::rc::Rc;
#[cfg(feature = "pa_v5_compatibility")]
use std::ffi::CString;
use std::ptr::null_mut;
use crate::def;
use crate::error::PAErr;
use crate::mainloop::api::{MainloopInternalType, MainloopInner, MainloopApi, Mainloop as MainloopTrait};
use crate::mainloop::signal::MainloopSignals;

pub use capi::pa_threaded_mainloop as MainloopInternal;

impl MainloopInternalType for MainloopInternal {}

/// This acts as a safe interface to the internal PA Mainloop.
///
/// The mainloop object pointers are further enclosed here in a ref counted wrapper, allowing this
/// outer wrapper to have clean methods for creating event objects, which can cleanly pass a copy of
/// the inner ref counted mainloop object to them. Giving this to events serves two purposes,
/// firstly because they need the API pointer, secondly, it ensures that event objects do not
/// outlive the mainloop object.
pub struct Mainloop {
    /// The ref-counted inner data.
    pub _inner: Rc<MainloopInner<MainloopInternal>>,
}

impl MainloopTrait for Mainloop {
    type MI = MainloopInner<MainloopInternal>;

    #[inline]
    fn inner(&self) -> Rc<MainloopInner<MainloopInternal>> {
        Rc::clone(&self._inner)
    }
}

impl MainloopSignals for Mainloop {}

impl MainloopInner<MainloopInternal> {
    fn drop_actual(&mut self) {
        unsafe { capi::pa_threaded_mainloop_free(self.ptr) };
        self.ptr = null_mut::<MainloopInternal>();
        self.api = null_mut::<MainloopApi>();
    }
}

impl Mainloop {
    /// Allocates a new threaded main loop object.
    ///
    /// You have to call [`start`](#method.start) before the event loop thread starts running.
    pub fn new() -> Option<Self> {
        let ptr = unsafe { capi::pa_threaded_mainloop_new() };
        if ptr.is_null() {
            return None;
        }
        let api_ptr = unsafe { capi::pa_threaded_mainloop_get_api(ptr) };
        assert!(!api_ptr.is_null());
        Some(
            Self {
                _inner: Rc::new(
                    MainloopInner::<MainloopInternal> {
                        ptr: ptr,
                        api: unsafe { std::mem::transmute(api_ptr) },
                        dropfn: MainloopInner::<MainloopInternal>::drop_actual,
                        supports_rtclock: true,
                    }
                ),
            }
        )
    }

    /// Starts the event loop thread.
    pub fn start(&mut self) -> Result<(), PAErr> {
        match unsafe { capi::pa_threaded_mainloop_start((*self._inner).ptr) } {
            0 => Ok(()),
            e => Err(PAErr(e)),
        }
    }

    /// Terminates the event loop thread cleanly.
    ///
    /// Make sure to unlock the mainloop object before calling this function.
    #[inline]
    pub fn stop(&mut self) {
        unsafe { capi::pa_threaded_mainloop_stop((*self._inner).ptr); }
    }

    /// Locks the event loop object, effectively blocking the event loop thread from processing
    /// events.
    ///
    /// You can use this to enforce exclusive access to all objects attached to the event loop. This
    /// lock is recursive. This function may not be called inside the event loop thread. Events that
    /// are dispatched from the event loop thread are executed with this lock held.
    #[inline]
    pub fn lock(&mut self) {
        assert!(!self.in_thread(), "lock() can not be called from within the event loop thread!");
        unsafe { capi::pa_threaded_mainloop_lock((*self._inner).ptr); }
    }

    /// Unlocks the event loop object, inverse of [`lock`](#method.lock).
    #[inline]
    pub fn unlock(&mut self) {
        unsafe { capi::pa_threaded_mainloop_unlock((*self._inner).ptr); }
    }

    /// Waits for an event to be signalled by the event loop thread.
    ///
    /// You can use this to pass data from the event loop thread to the main thread in a
    /// synchronized fashion. This function may not be called inside the event loop thread. Prior to
    /// this call the event loop object needs to be locked using [`lock`](#method.lock). While
    /// waiting the lock will be released. Immediately before returning it will be acquired again.
    /// This function may spuriously wake up even without [`signal`](#method.signal) being called.
    /// You need to make sure to handle that!
    #[inline]
    pub fn wait(&mut self) {
        unsafe { capi::pa_threaded_mainloop_wait((*self._inner).ptr); }
    }

    /// Signals all threads waiting for a signalling event in [`wait`](#method.wait).
    ///
    /// If `wait_for_accept` is non-zero, do not return before the signal was accepted by an
    /// [`accept`](#method.accept) call. While waiting for that condition the event loop object is
    /// unlocked.
    #[inline]
    pub fn signal(&mut self, wait_for_accept: bool) {
        unsafe { capi::pa_threaded_mainloop_signal((*self._inner).ptr, wait_for_accept as i32); }
    }

    /// Accepts a signal from the event thread issued with [`signal`].
    ///
    /// This call should only be used in conjunction with [`signal`] with `wait_for_accept` as
    /// `true`.
    ///
    /// [`signal`]: #method.signal
    #[inline]
    pub fn accept(&mut self) {
        unsafe { capi::pa_threaded_mainloop_accept((*self._inner).ptr); }
    }

    /// Gets the return value as specified with the main loop’s `quit` routine (used internally by
    /// threaded mainloop).
    #[inline]
    pub fn get_retval(&self) -> def::Retval {
        def::Retval(unsafe { capi::pa_threaded_mainloop_get_retval((*self._inner).ptr) })
    }

    /// Gets the main loop abstraction layer vtable for this main loop.
    ///
    /// There is no need to free this object as it is owned by the loop and is destroyed when the
    /// loop is freed.
    ///
    /// Talking to PA directly with C requires fetching this pointer explicitly via this function.
    /// This is actually unnecessary through this binding. The pointer is retrieved automatically
    /// upon Mainloop creation, stored internally, and automatically obtained from it by functions
    /// that need it.
    #[inline]
    pub fn get_api<'a>(&self) -> &'a MainloopApi {
        let ptr = (*self._inner).api;
        assert_eq!(false, ptr.is_null());
        unsafe { &*ptr }
    }

    /// Checks whether or not we are in the event loop thread (returns `true` if so).
    #[inline]
    pub fn in_thread(&self) -> bool {
        unsafe { capi::pa_threaded_mainloop_in_thread((*self._inner).ptr) != 0 }
    }

    /// Sets the name of the thread.
    ///
    /// Available since PA version 5.
    #[cfg(feature = "pa_v5_compatibility")]
    pub fn set_name(&mut self, name: &str) {
        // Warning: New CStrings will be immediately freed if not bound to a variable, leading to
        // as_ptr() giving dangling pointers!
        let c_name = CString::new(name.clone()).unwrap();
        unsafe { capi::pa_threaded_mainloop_set_name((*self._inner).ptr, c_name.as_ptr()); }
    }
}
