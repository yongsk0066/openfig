#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(improper_ctypes)]
#![allow(dead_code)]
#![allow(unexpected_cfgs)]
#![allow(clippy::all)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unnecessary_transmutes)]
// bindgen types size_t as c_ulong; on aarch64-apple-darwin that is the same as usize.
#![allow(suspicious_runtime_symbol_definitions)]
include!(concat!(env!("OUT_DIR"), "/nsworkspace.rs"));
