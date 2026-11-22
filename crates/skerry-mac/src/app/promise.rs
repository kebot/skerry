//! Dragging session files out to Finder as file promises (`ARCH-018`).
//!
//! GPUI's own external drag carries only files that already exist on the Mac, so the Files
//! window starts an AppKit dragging session itself: one `NSFilePromiseProvider` per selected
//! entry, whose delegate tells Finder the entry's name and, once Finder has picked the folder,
//! hands the destination to the Files window, which downloads into it and then calls the
//! completion handler. Promises are written on the main queue (AppKit's default), so the
//! completion handler never leaves the main thread.

use crate::selkies::{self, Entry};
use block2::{DynBlock, RcBlock};
use gpui_kit::{App, AsyncApp, Window};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    define_class, msg_send, AllocAnyThread as _, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSView, NSWorkspace,
};
use objc2_foundation::{NSError, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_uniform_type_identifiers::UTType;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Finder's completion handler for one promised file: call [`Completion::finish`] once.
pub struct Completion(RcBlock<dyn Fn(*mut NSError)>);

impl Completion {
    /// Tells Finder the file is written, or that writing it failed.
    pub fn finish(self, error: Option<&str>) {
        match error {
            None => self.0.call((std::ptr::null_mut(),)),
            Some(message) => {
                let error = file_error(message);
                self.0.call((Retained::as_ptr(&error).cast_mut(),));
            }
        }
    }
}

fn file_error(message: &str) -> Retained<NSError> {
    use objc2_foundation::{NSDictionary, NSLocalizedDescriptionKey};
    let key: &NSString = unsafe { NSLocalizedDescriptionKey };
    let message = NSString::from_str(message);
    let info = NSDictionary::<NSString, objc2::runtime::AnyObject>::from_slices(
        &[key],
        &[&*message as &objc2::runtime::AnyObject],
    );
    // NSFileWriteUnknownError in the Cocoa domain, which Finder reports with our message.
    unsafe {
        NSError::errorWithDomain_code_userInfo(
            &NSString::from_str("NSCocoaErrorDomain"),
            512,
            Some(&info),
        )
    }
}

/// A promise Finder asked to be written: download `entry` to `destination`.
pub struct Request {
    pub entry: Entry,
    pub destination: PathBuf,
    pub completion: Completion,
}

pub struct PromiseIvars {
    entry: Entry,
    requests: mpsc::UnboundedSender<Request>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SkerryFilePromise"]
    #[ivars = PromiseIvars]
    struct FilePromise;

    unsafe impl NSObjectProtocol for FilePromise {}

    unsafe impl NSFilePromiseProviderDelegate for FilePromise {
        #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
        fn file_name(
            &self,
            _provider: &NSFilePromiseProvider,
            _file_type: &NSString,
        ) -> Retained<NSString> {
            NSString::from_str(&self.ivars().entry.name)
        }

        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write_promise(
            &self,
            _provider: &NSFilePromiseProvider,
            url: &NSURL,
            completion_handler: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let completion = Completion(completion_handler.copy());
            let Some(destination) = url.path().map(|path| PathBuf::from(path.to_string())) else {
                completion.finish(Some("Finder gave no place to save the file."));
                return;
            };
            let request = Request {
                entry: self.ivars().entry.clone(),
                destination,
                completion,
            };
            if let Err(mpsc::error::SendError(request)) = self.ivars().requests.send(request) {
                request.completion.finish(Some("The Files window closed."));
            }
        }
    }

    unsafe impl NSDraggingSource for FilePromise {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            if context == NSDraggingContext::OutsideApplication {
                NSDragOperation::Copy
            } else {
                NSDragOperation::None
            }
        }
    }
);

impl FilePromise {
    fn new(
        entry: Entry,
        requests: mpsc::UnboundedSender<Request>,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PromiseIvars { entry, requests });
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// Providers hold their delegates weakly, so the delegates of the latest drags live here.
    static LIVE: RefCell<Vec<Retained<FilePromise>>> = const { RefCell::new(Vec::new()) };
}

/// Starts dragging `entries` out of `window` as file promises, from the mouse event AppKit is
/// handling now. `on_request` runs on the main thread for each promise Finder asks to have
/// written. Returns whether the drag started.
pub fn begin_drag(
    window: &mut Window,
    entries: &[Entry],
    on_request: impl Fn(Entry, PathBuf, Completion, &mut App) + 'static,
    cx: &mut App,
) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(view) = native_view(window) else {
        return false;
    };
    let Some(event) = NSApplication::sharedApplication(mtm).currentEvent() else {
        return false;
    };
    let entries: Vec<Entry> = entries
        .iter()
        .filter(|entry| selkies::local_name(&entry.name).is_some())
        .cloned()
        .collect();
    if entries.is_empty() {
        return false;
    }
    let (sender, mut receiver) = mpsc::unbounded_channel::<Request>();
    let point = view.convertPoint_fromView(event.locationInWindow(), None);
    let workspace = NSWorkspace::sharedWorkspace();
    let mut delegates = Vec::new();
    let mut items = Vec::new();
    for (index, entry) in entries.into_iter().enumerate() {
        let file_type = if entry.is_dir {
            "public.folder"
        } else {
            "public.data"
        };
        let icon_type = if entry.is_dir {
            UTType::typeWithIdentifier(&NSString::from_str("public.folder"))
        } else {
            entry.name.rsplit_once('.').and_then(|(_, extension)| {
                UTType::typeWithFilenameExtension(&NSString::from_str(extension))
            })
        }
        .or_else(|| UTType::typeWithIdentifier(&NSString::from_str("public.data")));
        let delegate = FilePromise::new(entry, sender.clone(), mtm);
        let provider = NSFilePromiseProvider::initWithFileType_delegate(
            NSFilePromiseProvider::alloc(),
            &NSString::from_str(file_type),
            ProtocolObject::from_ref(&*delegate),
        );
        let item = NSDraggingItem::initWithPasteboardWriter(
            NSDraggingItem::alloc(),
            ProtocolObject::from_ref(&*provider),
        );
        let offset = index as f64 * 6.;
        let frame = NSRect::new(
            NSPoint::new(point.x - 16. + offset, point.y - 16. - offset),
            NSSize::new(32., 32.),
        );
        let icon = icon_type.map(|kind| workspace.iconForContentType(&kind));
        // SAFETY: the contents are an NSImage, as the method expects.
        unsafe {
            item.setDraggingFrame_contents(
                frame,
                icon.as_deref()
                    .map(|icon| icon as &objc2::runtime::AnyObject),
            )
        };
        items.push(item);
        delegates.push(delegate);
    }
    let Some(source) = delegates.first().cloned() else {
        return false;
    };
    let items = objc2_foundation::NSArray::from_retained_slice(&items);
    view.beginDraggingSessionWithItems_event_source(
        &items,
        &event,
        ProtocolObject::from_ref(&*source),
    );
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        // Earlier drags have long been written; keep only this one's delegates alive.
        *live = delegates;
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Some(request) = receiver.recv().await {
            cx.update(|cx| on_request(request.entry, request.destination, request.completion, cx));
        }
    })
    .detach();
    true
}

fn native_view(window: &Window) -> Option<&NSView> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // GPUI supplies its live NSView; this runs on the main thread while the window exists.
    Some(unsafe { handle.ns_view.cast::<NSView>().as_ref() })
}
