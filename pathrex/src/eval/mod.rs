//! Generic abstractions over query evaluators.

use crate::graph::{GraphDecomposition, GraphError};

pub trait Evaluator {
    type Query;
    type Result;
    type Error;
    type Prepared: PreparedEvaluator<Result = Self::Result, Error = Self::Error>;

    fn prepare<G: GraphDecomposition>(
        &self,
        query: &Self::Query,
        graph: &G,
    ) -> Result<Self::Prepared, Self::Error>;

    fn evaluate<G: GraphDecomposition>(
        &self,
        query: &Self::Query,
        graph: &G,
    ) -> Result<Self::Result, Self::Error> {
        self.prepare(query, graph)?.execute()
    }
}

pub trait PreparedEvaluator {
    type Result;
    type Error;

    fn execute(&mut self) -> Result<Self::Result, Self::Error>;
}

pub trait ResultCount {
    fn result_count(&self) -> Result<usize, GraphError>;
}
