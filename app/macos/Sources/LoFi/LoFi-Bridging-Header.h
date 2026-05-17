// Swift bridging header — Swift sources see every declaration here as
// if it were native Swift. The only thing we need is the C API surface
// emitted by cbindgen from `app/core/src/ffi/`.
//
// The header itself is generated; its location is set via the
// `HEADER_SEARCH_PATHS` Xcode setting in `project.yml`.
#include "lofi_core.h"
