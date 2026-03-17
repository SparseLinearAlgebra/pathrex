use crate::graph::*;
use std::sync::Arc;

#[allow(dead_code)]
pub struct CountOutput<E: std::error::Error>(pub usize, std::marker::PhantomData<E>);

impl<E: std::error::Error> CountOutput<E> {
    #[allow(dead_code)]
    pub fn new(count: usize) -> Self {
        Self(count, std::marker::PhantomData)
    }
}

impl<E: std::error::Error> GraphDecomposition for CountOutput<E> {
    type Error = E;

    fn get_graph(&self, _label: &str) -> Result<Arc<LagraphGraph>, Self::Error> {
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
#[allow(dead_code)]
pub struct CountingBuilder<E: std::error::Error + Send + Sync + 'static>(
    pub usize,
    std::marker::PhantomData<E>,
);

impl<E: std::error::Error + Send + Sync + 'static> CountingBuilder<E> {
    #[allow(dead_code)]
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

impl<E: std::error::Error + Send + Sync + 'static> GraphSource<CountingBuilder<E>>
    for VecSource
{
    fn apply_to(
        self,
        mut builder: CountingBuilder<E>,
    ) -> Result<CountingBuilder<E>, E> {
        builder.0 += self.0.len();
        Ok(builder)
    }
}
