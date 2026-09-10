// Injected by the desktop app (crates/fig_desktop_api/src/init_script.rs) and
// mirrored from the Rust constants in fig_util::consts. The fallbacks keep
// these usable outside the app (vite dev server, vitest) where window.fig is
// not defined.
export const PRODUCT_NAME = window.fig?.constants?.productName ?? "Fig";
export const CLI_NAME = window.fig?.constants?.cliBinaryName ?? "q";
export const PTY_BINARY_NAME = window.fig?.constants?.ptyBinaryName ?? "figterm";
export const BUNDLE_ID = window.fig?.constants?.bundleId ?? "local.openfig";
export const URL_SCHEMA = window.fig?.constants?.urlSchema ?? "fig";
export const DATA_DIR_NAME = window.fig?.constants?.dataDirName ?? "fig";
