//! Runs the core's highlighter script (shiki, from `highlight.rs`) in the platform's JavaScript
//! engine. The core builds each call and interprets its answer; this module only evaluates them.
//! On a Mac the engine is JavaScriptCore through its C API. Other platforms have no engine yet,
//! so source stays plain there, as it does everywhere until the grammars compile.

use std::future::Future;

/// Resolves to the value `call`'s promise settles with, for `nr_syntax_answer`. The call comes
/// from `nr_opened_syntax_call`. It is ready on the first poll once the grammars have compiled,
/// slightly later while they compile, and never if the call fails.
pub fn settle(call: &str) -> impl Future<Output = String> + use<> {
    engine::settle(call)
}

#[cfg(target_os = "macos")]
mod engine {
    use crate::run::{Promise, Resolver, promise};
    use northroom_core::ffi::nr_syntax_script;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ffi::{CString, c_char, c_void};

    type Context = *mut c_void;
    type Value = *const c_void;
    type Object = *mut c_void;
    type JsString = *mut c_void;
    type Callback =
        unsafe extern "C" fn(Context, Object, Object, usize, *const Value, *mut Value) -> Value;

    #[link(name = "JavaScriptCore", kind = "framework")]
    unsafe extern "C" {
        fn JSGlobalContextCreate(class: *const c_void) -> Context;
        fn JSContextGetGlobalObject(context: Context) -> Object;
        fn JSStringCreateWithUTF8CString(text: *const c_char) -> JsString;
        fn JSStringRelease(text: JsString);
        fn JSStringGetMaximumUTF8CStringSize(text: JsString) -> usize;
        fn JSStringGetUTF8CString(text: JsString, buffer: *mut c_char, size: usize) -> usize;
        fn JSEvaluateScript(
            context: Context,
            script: JsString,
            this: Object,
            source: JsString,
            line: i32,
            exception: *mut Value,
        ) -> Value;
        fn JSObjectMakeFunctionWithCallback(
            context: Context,
            name: JsString,
            callback: Callback,
        ) -> Object;
        fn JSObjectSetProperty(
            context: Context,
            object: Object,
            name: JsString,
            value: Value,
            attributes: u32,
            exception: *mut Value,
        );
        fn JSValueToStringCopy(context: Context, value: Value, exception: *mut Value) -> JsString;
        fn JSValueToNumber(context: Context, value: Value, exception: *mut Value) -> f64;
        fn JSValueMakeUndefined(context: Context) -> Value;
    }

    /// Global function the script calls with `(call number, answer)`.
    const SETTLED: &str = "__nrSyntaxSettled";

    struct Engine {
        context: Context,
        /// Pending calls by number.
        waiting: HashMap<u64, Resolver<String>>,
        next: u64,
    }

    thread_local! {
        /// Started on first use; the grammars finish compiling shortly after.
        static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    }

    /// An owned `JSStringRef`, released on drop.
    struct Words(JsString);

    impl Words {
        fn of(text: &str) -> Self {
            let text = CString::new(text.replace('\0', "")).expect("no NUL is left");
            // SAFETY: a NUL-terminated UTF-8 string, copied by the call.
            Self(unsafe { JSStringCreateWithUTF8CString(text.as_ptr()) })
        }

        fn read(&self) -> String {
            // SAFETY: a live string, read into a buffer of the size it reports.
            unsafe {
                let size = JSStringGetMaximumUTF8CStringSize(self.0);
                let mut buffer = vec![0_u8; size];
                let written = JSStringGetUTF8CString(self.0, buffer.as_mut_ptr().cast(), size);
                buffer.truncate(written.saturating_sub(1));
                String::from_utf8_lossy(&buffer).into_owned()
            }
        }
    }

    impl Drop for Words {
        fn drop(&mut self) {
            // SAFETY: we own this string (from `of` or a copy) and release it once.
            unsafe { JSStringRelease(self.0) };
        }
    }

    /// Converts `value` to text as JavaScript's `String()` does.
    fn string_of(context: Context, value: Value) -> String {
        // SAFETY: a live value of `context`; the copy is released by `Words`.
        Words(unsafe { JSValueToStringCopy(context, value, std::ptr::null_mut()) }).read()
    }

    /// Evaluates `script`, logging any exception it throws.
    fn evaluate(context: Context, script: &str) {
        let script = Words::of(script);
        let mut thrown: Value = std::ptr::null();
        // SAFETY: a live context and string; the exception is written or left null.
        unsafe {
            JSEvaluateScript(
                context,
                script.0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut thrown,
            )
        };
        if !thrown.is_null() {
            eprintln!("syntax\t{}", string_of(context, thrown));
        }
    }

    /// `__nrSyntaxSettled(number, answer)`: resolves the pending call with that number.
    unsafe extern "C" fn settled(
        context: Context,
        _: Object,
        _: Object,
        count: usize,
        arguments: *const Value,
        _: *mut Value,
    ) -> Value {
        if count == 2 {
            // SAFETY: the engine hands `count` live arguments.
            let (number, answer) = unsafe { (*arguments, *arguments.add(1)) };
            // SAFETY: a live number value.
            let number = unsafe { JSValueToNumber(context, number, std::ptr::null_mut()) } as u64;
            let answer = string_of(context, answer);
            let waiting = ENGINE.with(|engine| {
                engine
                    .borrow_mut()
                    .as_mut()
                    .and_then(|engine| engine.waiting.remove(&number))
            });
            if let Some(waiting) = waiting {
                waiting.resolve(answer);
            }
        }
        // SAFETY: a live context.
        unsafe { JSValueMakeUndefined(context) }
    }

    fn start() -> Engine {
        // SAFETY: plain JavaScriptCore calls on the main thread; the context is kept for the
        // process.
        let context = unsafe { JSGlobalContextCreate(std::ptr::null()) };
        let name = Words::of(SETTLED);
        // SAFETY: as above; the function is kept by the global object.
        unsafe {
            let function = JSObjectMakeFunctionWithCallback(context, name.0, settled);
            JSObjectSetProperty(
                context,
                JSContextGetGlobalObject(context),
                name.0,
                function.cast_const(),
                0,
                std::ptr::null_mut(),
            );
        }
        evaluate(context, &crate::core::text(nr_syntax_script()));
        Engine {
            context,
            waiting: HashMap::new(),
            next: 0,
        }
    }

    pub fn settle(call: &str) -> Promise<String> {
        let (resolver, answer) = promise();
        let (context, number) = ENGINE.with(|engine| {
            let mut engine = engine.borrow_mut();
            let engine = engine.get_or_insert_with(start);
            let number = engine.next;
            engine.next += 1;
            engine.waiting.insert(number, resolver);
            (engine.context, number)
        });
        // Release the borrow first: a call that settles at once re-enters `ENGINE` in `settled`.
        evaluate(
            context,
            &format!("({call}).then((answer) => {SETTLED}({number}, String(answer)))"),
        );
        answer
    }
}

#[cfg(not(target_os = "macos"))]
mod engine {
    /// No engine on this platform yet, so source stays plain.
    pub fn settle(_: &str) -> std::future::Pending<String> {
        std::future::pending()
    }
}
