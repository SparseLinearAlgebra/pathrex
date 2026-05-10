use crate::graph::*;
use std::sync::Arc;

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
    ($grb_func:expr) => {{
        let info: $crate::lagraph_sys::GrB_Info = $grb_func.into();
        if info == $crate::lagraph_sys::GrB_Info::GrB_SUCCESS {
            Ok(())
        } else {
            Err($crate::graph::GraphError::GraphBlas(info))
        }
    }};
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
    ( $($func:ident)::+ ( $($arg:expr),* $(,)? ) ) => { {
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
        .with_stream(edges)
        .expect("Should insert edges stream")
        .build()
        .expect("build must succeed")
}
