//! Linker definitions for the Python C API symbols that type_kernel's
//! reachable closure references from the pure `is_subtype` path.
//!
//! Why this exists: `is_subtype`'s protocol branches consult mypy's live
//! typestate map through pyo3 (`Python::with_gil` in subtypes.rs,
//! protocols.rs, constraints.rs). The skeleton never installs a live map,
//! so those branches are guarded off at runtime and never run, but the
//! code carrying their `Py_*` references is linked into this binary
//! unconditionally, and dyld binds every undefined reference eagerly
//! before `main` (chained fixups admit no lazy binding), so a binary
//! without these definitions cannot even launch.
//!
//! Defining the symbols locally gives the linker a static answer: the
//! binary carries zero undefined Python imports, links no libpython, and
//! starts no interpreter. A stub that somehow executes aborts with its
//! own name, so an accidental Python path fails loudly in the first test
//! run instead of silently degrading. The gates test asserts the stronger
//! invariant (`nm -u` shows no Py symbols at all).
//!
//! The list is the exact set of undefined `Py*`/`_Py*` symbols in the
//! linked binary; the link has no `-undefined dynamic_lookup` escape
//! hatch, so any new symbol type_kernel starts referencing fails the
//! build and must be added here.
//!
//! Every function below carries its real C prototype, transcribed from
//! the pyo3-ffi 0.20.3 declarations the linked pyo3 code calls through
//! (same crate version, same cfg set, resolved by the workspace lock).
//! The stub bodies still diverge before any argument or return value
//! crosses the boundary, but the parameter and return shapes now match
//! the declarations, so a guarded call is well-defined up to the abort
//! instead of reading garbage from return registers or mismatching the
//! sret convention. Each stub is emitted together with a `const` that
//! coerces pyo3-ffi's own declaration of that symbol to the same
//! fn-pointer type: if a prototype here ever disagrees with the
//! registry, the build fails at that line. That guard is why the
//! signatures are hand-transcribed rather than code-generated; it
//! checks against the compiled pyo3-ffi of this exact build, which a
//! registry-parsing generator could only approximate, and it has no
//! moving parts.
//!
//! The data symbols carry storage sized and aligned for the real
//! CPython objects, not one-size cells: a reached path that reads
//! through them (e.g. `tp_flags` at offset 168) must stay inside the
//! cell. The sizes come from the CPython 3.12 and 3.13 headers, which
//! measure identically on LP64 (#94 records the measurement): PyObject
//! 16, PyLongObject 32, PyTypeObject 416, all
//! align 8, and they bound every layout in the abi3-py37 window this
//! binary targets. The pyo3-ffi build is abi3, under which PyTypeObject
//! is an opaque zero-sized marker, so the object-shaped cells derive
//! their sizes from the exported PyObject/PyVarObject structs and the
//! type-object cells carry the measured constant. Data symbols are
//! tied to pyo3-ffi's declarations where the types allow it: the
//! `data_static_type_guard` below binds `PyBool_Type` and `PySet_Type`,
//! the only struct-shaped data statics pyo3-ffi exports, and the tie
//! checks existence and declared type, not size, since abi3 hides the
//! layout. The `_Py_*Struct` singletons are crate-private in pyo3-ffi
//! and admit no tie, so their cells are covered only by the derived
//! and measured constants. The LP64 assumption behind the measured
//! numbers is enforced by the `compile_error!` cfg gate above. Only
//! the addresses of these symbols are taken on the guarded paths; the
//! sizing removes the out-of-bounds read a reached path would
//! otherwise perform.

#![allow(dead_code)]

use std::mem::size_of;
use std::os::raw::{c_char, c_int, c_long, c_ulong};

use pyo3_ffi::{PyGILState_STATE, PyObject, PyThreadState, PyTypeObject, PyVarObject, Py_ssize_t};

#[cfg(not(target_pointer_width = "64"))]
compile_error!(
    "python_stubs data cells are sized for LP64 (measured on CPython \
     3.12 and 3.13, #94); a non-64-bit target would mis-size them"
);

fn stub_hit(name: &str) -> ! {
    eprintln!("fatal: python-free skeleton executed {name}, a Python C API stub");
    std::process::abort()
}

/// Emits one C API stub plus the compile-time signature guard described
/// in the module docs: the const forces pyo3-ffi's declaration of the
/// symbol to unify with the prototype spelled here.
macro_rules! py_fn_stub {
    ($($name:ident($($arg:ty),* $(,)?) -> $ret:ty),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "C" fn $name($(_: $arg),*) -> $ret {
                stub_hit(stringify!($name))
            }
            const _: unsafe extern "C" fn($($arg),*) -> $ret = pyo3_ffi::$name;
        )*
    };
}

py_fn_stub!(
    _Py_Dealloc(*mut PyObject) -> (),
    Py_InitializeEx(c_int) -> (),
    Py_IsInitialized() -> c_int,
    PyBytes_AsString(*mut PyObject) -> *mut c_char,
    PyBytes_FromStringAndSize(*const c_char, Py_ssize_t) -> *mut PyObject,
    PyBytes_Size(*mut PyObject) -> Py_ssize_t,
    PyDict_Contains(*mut PyObject, *mut PyObject) -> c_int,
    PyDict_GetItemWithError(*mut PyObject, *mut PyObject) -> *mut PyObject,
    PyErr_Fetch(
        *mut *mut PyObject,
        *mut *mut PyObject,
        *mut *mut PyObject,
    ) -> (),
    PyErr_NewExceptionWithDoc(
        *const c_char,
        *const c_char,
        *mut PyObject,
        *mut PyObject,
    ) -> *mut PyObject,
    PyErr_NormalizeException(
        *mut *mut PyObject,
        *mut *mut PyObject,
        *mut *mut PyObject,
    ) -> (),
    PyErr_Print() -> (),
    PyErr_PrintEx(c_int) -> (),
    PyErr_Restore(*mut PyObject, *mut PyObject, *mut PyObject) -> (),
    PyErr_SetObject(*mut PyObject, *mut PyObject) -> (),
    PyErr_SetString(*mut PyObject, *const c_char) -> (),
    PyEval_SaveThread() -> *mut PyThreadState,
    PyGILState_Ensure() -> PyGILState_STATE,
    PyGILState_Release(PyGILState_STATE) -> (),
    PyImport_Import(*mut PyObject) -> *mut PyObject,
    PyIter_Next(*mut PyObject) -> *mut PyObject,
    PyList_GetItem(*mut PyObject, Py_ssize_t) -> *mut PyObject,
    PyList_Size(*mut PyObject) -> Py_ssize_t,
    PyLong_AsLong(*mut PyObject) -> c_long,
    PyNumber_Index(*mut PyObject) -> *mut PyObject,
    PyObject_Call(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject,
    PyObject_GetAttr(*mut PyObject, *mut PyObject) -> *mut PyObject,
    PyObject_GetItem(*mut PyObject, *mut PyObject) -> *mut PyObject,
    PyObject_GetIter(*mut PyObject) -> *mut PyObject,
    PyObject_IsInstance(*mut PyObject, *mut PyObject) -> c_int,
    PyObject_IsTrue(*mut PyObject) -> c_int,
    PyObject_Repr(*mut PyObject) -> *mut PyObject,
    PyObject_Size(*mut PyObject) -> Py_ssize_t,
    PyObject_Str(*mut PyObject) -> *mut PyObject,
    PySequence_Check(*mut PyObject) -> c_int,
    PySequence_Size(*mut PyObject) -> Py_ssize_t,
    PySet_Contains(*mut PyObject, *mut PyObject) -> c_int,
    PyTuple_New(Py_ssize_t) -> *mut PyObject,
    PyTuple_SetItem(*mut PyObject, Py_ssize_t, *mut PyObject) -> c_int,
    PyType_GetFlags(*mut PyTypeObject) -> c_ulong,
    PyType_IsSubtype(*mut PyTypeObject, *mut PyTypeObject) -> c_int,
    PyUnicode_AsEncodedString(
        *mut PyObject,
        *const c_char,
        *const c_char,
    ) -> *mut PyObject,
    PyUnicode_AsUTF8String(*mut PyObject) -> *mut PyObject,
    PyUnicode_FromStringAndSize(*const c_char, Py_ssize_t) -> *mut PyObject,
    PyUnicode_InternInPlace(*mut *mut PyObject) -> (),
);

/// The C symbols are `PyObject *` exception slots: one pointer-sized
/// zeroed cell each, the exact shape pyo3-ffi declares for them.
#[repr(C, align(8))]
pub struct PyObjectPtrCell([u8; PY_POINTER_CELL]);

const PY_POINTER_CELL: usize = size_of::<*mut PyObject>();

#[no_mangle]
pub static PyExc_BaseException: PyObjectPtrCell = PyObjectPtrCell([0; PY_POINTER_CELL]);
#[no_mangle]
pub static PyExc_OverflowError: PyObjectPtrCell = PyObjectPtrCell([0; PY_POINTER_CELL]);
#[no_mangle]
pub static PyExc_TypeError: PyObjectPtrCell = PyObjectPtrCell([0; PY_POINTER_CELL]);

/// PyLongObject is a PyVarObject head plus one 32-bit digit (CPython
/// longintrepr.h); the align(8) cell rounds the 28-byte payload to the
/// measured 32.
const PY_LONG_OBJECT_CELL: usize = size_of::<PyVarObject>() + size_of::<u32>();

/// sizeof(PyTypeObject) on the CPython 3.12 and 3.13 headers, LP64:
/// pyo3-ffi's abi3 build hides the layout, so this one is measured,
/// not derived.
const PY_TYPE_OBJECT_CELL: usize = 416;

/// Storage cells for the struct-shaped data symbols. Sizing derives
/// from pyo3-ffi's exported structs where it exports the real layout,
/// and from the measured CPython constants where the abi3 build only
/// exposes an opaque marker.
#[repr(C, align(8))]
pub struct PyObjectCell([u8; size_of::<PyObject>()]);

#[repr(C, align(8))]
pub struct PyLongObjectCell([u8; PY_LONG_OBJECT_CELL]);

#[repr(C, align(8))]
pub struct PyTypeObjectCell([u8; PY_TYPE_OBJECT_CELL]);

const _: () = assert!(size_of::<PyObjectCell>() == 16 && size_of::<PyLongObjectCell>() == 32);
const _: () = assert!(size_of::<PyTypeObjectCell>() == 416);

#[no_mangle]
pub static _Py_FalseStruct: PyLongObjectCell = PyLongObjectCell([0; PY_LONG_OBJECT_CELL]);
#[no_mangle]
pub static _Py_NoneStruct: PyObjectCell = PyObjectCell([0; size_of::<PyObject>()]);
#[no_mangle]
pub static _Py_TrueStruct: PyLongObjectCell = PyLongObjectCell([0; PY_LONG_OBJECT_CELL]);
#[no_mangle]
pub static PyBool_Type: PyTypeObjectCell = PyTypeObjectCell([0; PY_TYPE_OBJECT_CELL]);
#[no_mangle]
pub static PySet_Type: PyTypeObjectCell = PyTypeObjectCell([0; PY_TYPE_OBJECT_CELL]);

/// Compile-time tie from the type-object data symbols to pyo3-ffi's
/// own declarations, the data-symbol analogue of the fn guards in
/// `py_fn_stub!`: the coercions below only compile while pyo3-ffi
/// exports `PyBool_Type` and `PySet_Type` as `PyTypeObject`-typed
/// statics. Const items cannot reference statics, so the check lives in
/// a never-called function; it runs at typecheck time, which is the
/// part that matters. Under abi3 `PyTypeObject` is an opaque marker, so
/// this ties existence and declared type, never a size; the `_Py_`
/// singletons are crate-private in pyo3-ffi and cannot be tied at all.
fn data_static_type_guard() -> [*const PyTypeObject; 2] {
    [
        std::ptr::addr_of!(pyo3_ffi::PyBool_Type),
        std::ptr::addr_of!(pyo3_ffi::PySet_Type),
    ]
}
