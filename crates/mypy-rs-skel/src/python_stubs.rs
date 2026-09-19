//! Linker definitions for the Python C API symbols that type_kernel's
//! reachable closure references from the pure `is_subtype` path.
//!
//! Why this exists: `is_subtype`'s protocol branches consult mypy's live
//! typestate map through pyo3 (`Python::with_gil` in subtypes.rs,
//! protocols.rs, constraints.rs). The skeleton never installs a live map,
//! so those branches are guarded off at runtime and never run — but the
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

#![allow(dead_code)]

fn stub_hit(name: &str) -> ! {
    eprintln!("fatal: python-free skeleton executed {name}, a Python C API stub");
    std::process::abort()
}

macro_rules! py_fn_stub {
    ($($name:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub extern "C" fn $name() {
                stub_hit(stringify!($name))
            }
        )*
    };
}

py_fn_stub!(
    _Py_Dealloc,
    Py_InitializeEx,
    Py_IsInitialized,
    PyBytes_AsString,
    PyBytes_FromStringAndSize,
    PyBytes_Size,
    PyDict_Contains,
    PyDict_GetItemWithError,
    PyErr_Fetch,
    PyErr_NewExceptionWithDoc,
    PyErr_NormalizeException,
    PyErr_Print,
    PyErr_PrintEx,
    PyErr_Restore,
    PyErr_SetObject,
    PyErr_SetString,
    PyEval_SaveThread,
    PyGILState_Ensure,
    PyGILState_Release,
    PyImport_Import,
    PyIter_Next,
    PyList_GetItem,
    PyList_Size,
    PyLong_AsLong,
    PyNumber_Index,
    PyObject_Call,
    PyObject_GetAttr,
    PyObject_GetItem,
    PyObject_GetIter,
    PyObject_IsInstance,
    PyObject_IsTrue,
    PyObject_Repr,
    PyObject_Size,
    PyObject_Str,
    PySequence_Check,
    PySequence_Size,
    PySet_Contains,
    PyTuple_New,
    PyTuple_SetItem,
    PyType_GetFlags,
    PyType_IsSubtype,
    PyUnicode_AsEncodedString,
    PyUnicode_AsUTF8String,
    PyUnicode_FromStringAndSize,
    PyUnicode_InternInPlace,
);

/// Data symbols: only their addresses are taken by the linked protocol
/// code (comparisons against `&PyBool_Type`, reads of `PyExc_*`), all on
/// paths the skeleton never executes. A plain zero-sized-in-spirit cell
/// satisfies the address; nothing ever reads through it.
macro_rules! py_static_stub {
    ($($name:ident),* $(,)?) => {
        $(
            #[no_mangle]
            pub static $name: u64 = 0;
        )*
    };
}

py_static_stub!(
    _Py_FalseStruct,
    _Py_NoneStruct,
    _Py_TrueStruct,
    PyBool_Type,
    PyExc_BaseException,
    PyExc_OverflowError,
    PyExc_TypeError,
    PySet_Type,
);
