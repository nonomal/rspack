use std::cell::RefCell;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::{fmt::Debug, sync::LazyLock};

use napi::bindgen_prelude::TypeName;
use napi::ValueType;
use napi::{
  bindgen_prelude::{FromNapiValue, Function, ToNapiValue, ValidateNapiValue},
  Either, Env, NapiRaw, Ref,
};
use rspack_core::{Filename, FilenameFn};
use rspack_core::{LocalFilenameFn, PathData, PublicPath};
use rspack_napi::threadsafe_function::ThreadsafeFunction;
use serde::Deserialize;

use crate::{AssetInfo, JsPathData};

thread_local! {
  static COMMUNICATION_MODE: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

  static JS_FILENAME_FN: RefCell<Option<Ref<Function<'static, (JsPathData, Option<AssetInfo>), String>>>> = RefCell::new(None);
}

pub static CHANNEL: LazyLock<(
  crossbeam::channel::Sender<(JsPathData, Option<AssetInfo>, oneshot::Sender<String>)>,
  crossbeam::channel::Receiver<(JsPathData, Option<AssetInfo>, oneshot::Sender<String>)>,
)> = LazyLock::new(|| {
  let (sender, receiver) =
    crossbeam::channel::unbounded::<(JsPathData, Option<AssetInfo>, oneshot::Sender<String>)>();
  (sender, receiver)
});

pub fn wait_function_call<R: Send + 'static>(
  env: Env,
  f: impl FnOnce() -> napi::Result<R> + Send,
) -> napi::Result<R> {
  println!(
    "new wait_function_call id {:?}",
    std::thread::current().id()
  );
  COMMUNICATION_MODE.with(|cm| {
    cm.store(true, std::sync::atomic::Ordering::Relaxed);
  });
  let (sender, receiver) = oneshot::channel::<napi::Result<R>>();
  std::thread::scope(|s| {
    s.spawn(move || sender.send(f()));

    loop {
      if let Ok((path_data, asset_info, result_sender)) = CHANNEL.1.try_recv() {
        let js_fn = JS_FILENAME_FN.with(|ref_cell| {
          let js_fn_ref = ref_cell.borrow();
          js_fn_ref.as_ref().unwrap().get_value(&env).unwrap()
        });
        println!("call js function");
        let result = js_fn.call((path_data, asset_info)).unwrap();
        println!("send result {}", result);
        result_sender.send(result);
      }

      if let Ok(result) = receiver.try_recv() {
        COMMUNICATION_MODE.with(|cm| {
          cm.store(false, std::sync::atomic::Ordering::Relaxed);
        });
        println!("wait_function_call end");
        break result;
      }
    }
  })
}

#[derive(Debug)]
pub struct JsFilenameFn {
  communication_mode: Arc<AtomicBool>,
  sender: crossbeam::channel::Sender<(JsPathData, Option<AssetInfo>, oneshot::Sender<String>)>,
  ts: ThreadsafeFunction<(JsPathData, Option<AssetInfo>), String>,
}

impl JsFilenameFn {
  pub fn new(
    env: Env,
    js: Function<(JsPathData, Option<AssetInfo>), String>,
  ) -> napi::Result<Self> {
    println!("new thread id {:?}", std::thread::current().id());
    let communication_mode = COMMUNICATION_MODE.with(|cm| cm.clone());
    let ts = unsafe { ThreadsafeFunction::from_napi_value(env.raw(), js.raw())? };
    let js = unsafe {
      std::mem::transmute::<
        &Function<(JsPathData, Option<AssetInfo>), String>,
        &'static Function<(JsPathData, Option<AssetInfo>), String>,
      >(&js)
    };
    let js = Ref::new(&env, js)?;
    JS_FILENAME_FN.with(|ref_cell| {
      let mut js_filename = ref_cell.borrow_mut();
      *(&mut *js_filename) = Some(js);
    });
    Ok(Self {
      communication_mode,
      sender: CHANNEL.0.clone(),
      ts,
    })
  }

  pub fn call(
    &self,
    path_data: &PathData,
    asset_info: Option<&rspack_core::AssetInfo>,
  ) -> napi::Result<String> {
    if self
      .communication_mode
      .load(std::sync::atomic::Ordering::Relaxed)
    {
      println!("call with channel");
      let js_path_data = JsPathData::from_path_data(*path_data);
      let js_asset_info = asset_info.cloned().map(AssetInfo::from);
      let (result_sender, result_receiver) = oneshot::channel();
      self
        .sender
        .send((js_path_data, js_asset_info, result_sender));

      Ok(result_receiver.recv().unwrap())
    } else {
      println!("blocking_call_with_sync");
      Ok(
        self
          .ts
          .blocking_call_with_sync((
            JsPathData::from_path_data(*path_data),
            asset_info.cloned().map(AssetInfo::from),
          ))
          .unwrap(),
      )
    }
  }
}

impl FromNapiValue for JsFilenameFn {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    napi_val: napi::sys::napi_value,
  ) -> napi::Result<Self> {
    let js_fn =
      Function::<(JsPathData, Option<AssetInfo>), String>::from_napi_value(env, napi_val)?;
    let env_wrapper = Env::from_raw(env);
    Ok(Self::new(env_wrapper, js_fn)?)
  }
}

impl TypeName for JsFilenameFn {
  fn type_name() -> &'static str {
    "JsFilename"
  }

  fn value_type() -> napi::ValueType {
    ValueType::Function
  }
}

impl ValidateNapiValue for JsFilenameFn {}

/// A js filename value. Either a string or a function
///
/// The function type is generic. By default the function type is tsfn.
#[derive(Debug)]
pub struct JsFilename<F = JsFilenameFn>(Either<String, F>);

/// A local js filename value. Only valid in the current native call.
///
/// Useful as the type of a parameter that is invoked immediately inside the function.
pub type LocalJsFilename<'f> = JsFilename<Function<'f, (JsPathData, Option<AssetInfo>), String>>;

impl<'f> From<LocalJsFilename<'f>> for Filename<LocalJsFilenameFn<'f>> {
  fn from(value: LocalJsFilename<'f>) -> Self {
    match value.0 {
      Either::A(template) => Filename::from(template),
      Either::B(js_func) => Filename::from_fn(LocalJsFilenameFn(js_func)),
    }
  }
}
impl From<JsFilename> for Filename {
  fn from(value: JsFilename) -> Self {
    match value.0 {
      Either::A(template) => Filename::from(template),
      Either::B(theadsafe_filename_fn) => {
        Filename::from_fn(Arc::new(ThreadSafeFilenameFn(theadsafe_filename_fn)))
      }
    }
  }
}

impl From<JsFilename> for PublicPath {
  fn from(value: JsFilename) -> Self {
    match value.0 {
      Either::A(template) => template.into(),
      Either::B(theadsafe_filename_fn) => PublicPath::Filename(Filename::from_fn(Arc::new(
        ThreadSafeFilenameFn(theadsafe_filename_fn),
      ))),
    }
  }
}

impl<F: FromNapiValue + ValidateNapiValue> FromNapiValue for JsFilename<F> {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    napi_val: napi::sys::napi_value,
  ) -> napi::Result<Self> {
    Ok(Self(Either::from_napi_value(env, napi_val)?))
  }
}

impl<F: ToNapiValue> ToNapiValue for JsFilename<F> {
  unsafe fn to_napi_value(
    env: napi::sys::napi_env,
    val: Self,
  ) -> napi::Result<napi::sys::napi_value> {
    Either::to_napi_value(env, val.0)
  }
}

impl<'de> Deserialize<'de> for JsFilename {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: serde::Deserializer<'de>,
  {
    Ok(Self(Either::A(String::deserialize(deserializer)?)))
  }
}

/// Wrapper of a thread-safe filename js function. Implements `FilenameFn`
#[derive(Debug)]
struct ThreadSafeFilenameFn(JsFilenameFn);
impl LocalFilenameFn for ThreadSafeFilenameFn {
  type Error = rspack_error::Error;
  fn call(
    &self,
    path_data: &PathData,
    asset_info: Option<&rspack_core::AssetInfo>,
  ) -> rspack_error::Result<String> {
    let result = self.0.call(path_data, asset_info).unwrap();
    Ok(result)
  }
}
impl FilenameFn for ThreadSafeFilenameFn {}

/// Wrapper of a local filename js function. Implements `LocalFilenameFn`. Only valid in the current native call.
pub struct LocalJsFilenameFn<'f>(Function<'f, (JsPathData, Option<AssetInfo>), String>);

impl LocalFilenameFn for LocalJsFilenameFn<'_> {
  type Error = napi::Error;

  fn call(
    &self,
    path_data: &PathData,
    asset_info: Option<&rspack_core::AssetInfo>,
  ) -> Result<String, Self::Error> {
    let js_path_data = JsPathData::from_path_data(*path_data);
    let js_asset_info = asset_info.cloned().map(AssetInfo::from);
    self.0.call((js_path_data, js_asset_info))
  }
}
