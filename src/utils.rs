use crate::{graph::*, lagraph_sys::*};
use std::{fmt::Display, sync::Arc};

pub struct CountOutput<E: std::error::Error>(pub usize, std::marker::PhantomData<E>);

impl<E: std::error::Error> CountOutput<E> {
    #[allow(dead_code)]
    pub fn new(count: usize) -> Self {
        Self(count, std::marker::PhantomData)
    }
}

impl<E: std::error::Error> GraphDecomposition for CountOutput<E> {
    fn get_graph(&self, _label: &str) -> Result<Arc<LagraphGraph>, GraphError> {
        unimplemented!("CountOutput is a test stub")
    }

    fn get_node_id(&self, _string_id: &str) -> Option<usize> {
        None
    }

    fn get_node_name(&self, _mapped_id: usize) -> Option<String> {
        None
    }

    fn num_nodes(&self) -> usize {
        self.0
    }
}

/// A minimal [`GraphBuilder`] that counts pushed edges and produces a [`CountOutput`].
pub struct CountingBuilder<E: std::error::Error + Send + Sync + 'static>(
    pub usize,
    std::marker::PhantomData<E>,
);

impl<E: std::error::Error + Send + Sync + 'static> CountingBuilder<E> {
    pub fn new() -> Self {
        Self(0, std::marker::PhantomData)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> Default for CountingBuilder<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: std::error::Error + Send + Sync + 'static> GraphBuilder for CountingBuilder<E> {
    type Graph = CountOutput<E>;
    type Error = E;

    fn build(self) -> Result<Self::Graph, Self::Error> {
        Ok(CountOutput::new(self.0))
    }
}

#[allow(dead_code)]
pub struct VecSource(pub Vec<Edge>);

impl<E: std::error::Error + Send + Sync + 'static> GraphSource<CountingBuilder<E>> for VecSource {
    fn apply_to(self, mut builder: CountingBuilder<E>) -> Result<CountingBuilder<E>, E> {
        builder.0 += self.0.len();
        Ok(builder)
    }
}

impl Display for GrB_Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrB_Info::GrB_SUCCESS => write!(f, "GrB_SUCCESS"),
            GrB_Info::GrB_NO_VALUE => write!(f, "GrB_NO_VALUE"),
            GrB_Info::GxB_EXHAUSTED => write!(f, "GxB_EXHAUSTED"),
            GrB_Info::GrB_UNINITIALIZED_OBJECT => write!(f, "GrB_UNINITIALIZED_OBJECT"),
            GrB_Info::GrB_NULL_POINTER => write!(f, "GrB_NULL_POINTER"),
            GrB_Info::GrB_INVALID_VALUE => write!(f, "GrB_INVALID_VALUE"),
            GrB_Info::GrB_INVALID_INDEX => write!(f, "GrB_INVALID_INDEX"),
            GrB_Info::GrB_DOMAIN_MISMATCH => write!(f, "GrB_DOMAIN_MISMATCH"),
            GrB_Info::GrB_DIMENSION_MISMATCH => write!(f, "GrB_DIMENSION_MISMATCH"),
            GrB_Info::GrB_OUTPUT_NOT_EMPTY => write!(f, "GrB_OUTPUT_NOT_EMPTY"),
            GrB_Info::GrB_NOT_IMPLEMENTED => write!(f, "GrB_NOT_IMPLEMENTED"),
            GrB_Info::GrB_ALREADY_SET => write!(f, "GrB_ALREADY_SET"),
            GrB_Info::GrB_PANIC => write!(f, "GrB_PANIC"),
            GrB_Info::GrB_OUT_OF_MEMORY => write!(f, "GrB_OUT_OF_MEMORY"),
            GrB_Info::GrB_INSUFFICIENT_SPACE => write!(f, "GrB_INSUFFICIENT_SPACE"),
            GrB_Info::GrB_INVALID_OBJECT => write!(f, "GrB_INVALID_OBJECT"),
            GrB_Info::GrB_INDEX_OUT_OF_BOUNDS => write!(f, "GrB_INDEX_OUT_OF_BOUNDS"),
            GrB_Info::GrB_EMPTY_OBJECT => write!(f, "GrB_EMPTY_OBJECT"),
            GrB_Info::GxB_JIT_ERROR => write!(f, "GxB_JIT_ERROR"),
            GrB_Info::GxB_GPU_ERROR => write!(f, "GxB_GPU_ERROR"),
            GrB_Info::GxB_OUTPUT_IS_READONLY => write!(f, "GxB_OUTPUT_IS_READONLY"),
        }
    }
}

impl From<i32> for GrB_Info {
    fn from(value: i32) -> Self {
        match value {
            0 => GrB_Info::GrB_SUCCESS,
            1 => GrB_Info::GrB_NO_VALUE,
            7 => GrB_Info::GxB_EXHAUSTED,
            -1 => GrB_Info::GrB_UNINITIALIZED_OBJECT,
            -2 => GrB_Info::GrB_NULL_POINTER,
            -3 => GrB_Info::GrB_INVALID_VALUE,
            -4 => GrB_Info::GrB_INVALID_INDEX,
            -5 => GrB_Info::GrB_DOMAIN_MISMATCH,
            -6 => GrB_Info::GrB_DIMENSION_MISMATCH,
            -7 => GrB_Info::GrB_OUTPUT_NOT_EMPTY,
            -8 => GrB_Info::GrB_NOT_IMPLEMENTED,
            -9 => GrB_Info::GrB_ALREADY_SET,
            -101 => GrB_Info::GrB_PANIC,
            -102 => GrB_Info::GrB_OUT_OF_MEMORY,
            -103 => GrB_Info::GrB_INSUFFICIENT_SPACE,
            -104 => GrB_Info::GrB_INVALID_OBJECT,
            -105 => GrB_Info::GrB_INDEX_OUT_OF_BOUNDS,
            -106 => GrB_Info::GrB_EMPTY_OBJECT,
            -7001 => GrB_Info::GxB_JIT_ERROR,
            -7002 => GrB_Info::GxB_GPU_ERROR,
            -7003 => GrB_Info::GxB_OUTPUT_IS_READONLY,
            _ => unimplemented!("Hope no more GrB status codes!"),
        }
    }
}

/// Calls a raw GraphBLAS function expression and maps its `i32` return code to
/// `Result<(), GraphError>`.
///
/// The expression is evaluated inside an `unsafe` block; the caller is
/// responsible for ensuring all pointer arguments are valid.
///
/// # Example
/// ```ignore
/// grb_ok!(GrB_Matrix_new(&mut mat, GrB_BOOL, rows, cols))?;
/// ```
#[macro_export]
macro_rules! grb_ok {
    ($grb_func:expr) => {
        unsafe {
            let info: $crate::lagraph_sys::GrB_Info = $grb_func.into();
            if info == $crate::lagraph_sys::GrB_Info::GrB_SUCCESS {
                Ok(())
            } else {
                Err($crate::graph::GraphError::GraphBlas(info))
            }
        }
    };
}

/// Calls a raw LAGraph function and maps its `i32` return code to
/// `Result<(), GraphError>`.
///
/// LAGraph functions take an extra trailing `*mut i8` message buffer as their
/// last argument. This macro allocates that buffer on the stack, appends it
/// automatically, and — on failure — reads the null-terminated error string
/// from the buffer and attaches it to [`GraphError::LAGraph`].
///
/// The entire body runs inside an `unsafe` block; the caller is responsible
/// for ensuring all other pointer arguments are valid.
///
/// # Example
/// ```ignore
/// la_ok!(LAGraph_Init())?; // not la_ok!(LAGraph_Init(msg))?;
/// ```
#[macro_export]
macro_rules! la_ok {
    ( $($func:ident)::+ ( $($arg:expr),* $(,)? ) ) => { unsafe {
        let mut msg = [0i8; $crate::lagraph_sys::LAGRAPH_MSG_LEN as usize];
        let info: $crate::lagraph_sys::GrB_Info =
            $($func)::+($($arg,)* msg.as_mut_ptr()).into();
        if info == $crate::lagraph_sys::GrB_Info::GrB_SUCCESS {
            Ok(())
        } else {
            let msg_str =
                ::std::ffi::CStr::from_ptr(msg.as_ptr())
                    .to_string_lossy()
                    .into_owned();
            Err($crate::graph::GraphError::LAGraph(info, msg_str))
        }
    }};
}

pub fn build_graph(edges: &[(&str, &str, &str)]) -> <InMemory as Backend>::Graph {
    let builder = InMemoryBuilder::new();
    let edges = edges
        .iter()
        .cloned()
        .map(|(s, t, l)| {
            Ok(Edge {
                source: s.to_string(),
                label: l.to_string(),
                target: t.to_string(),
            })
        })
        .collect::<Vec<Result<Edge, GraphError>>>();
    builder
        .with_stream(edges.into_iter())
        .expect("Should insert edges stream")
        .build()
        .expect("build must succeed")
}
