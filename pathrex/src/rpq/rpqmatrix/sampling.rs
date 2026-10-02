use std::sync::Arc;

use rand::{SeedableRng, rngs::StdRng, seq::index};

use crate::graph::GraphblasMatrix;

use crate::lagraph_sys::{
    GrB_Info, GrB_Matrix, GrB_Matrix_nvals,
    LAGraph_RPQMatrix_label, LAGraph_RPQMatrix_sample_apply, LAGraph_RPQMatrix_sample_identity,
    LAGraph_RPQMatrix_sample_stats, LAGraph_RPQMatrix_sample_submatrix,
    LAGraph_RPQMatrix_sample_union,
};

#[derive(Clone, Copy)]
pub(super) struct SamplingConfig {
    pub percent: usize,
    pub seed: u64,
    pub max_star_iterations: usize,
}

#[derive(Clone, Copy, Default)]
pub(super) struct SampleEstimate {
    pub nnz: f64,
    pub rows: f64,
    pub cols: f64,
    pub exact: bool,
    pub converged: bool,
}

fn sample_matrix(info: GrB_Info, raw: GrB_Matrix) -> Option<Arc<GraphblasMatrix>> {
    let matrix = GraphblasMatrix::from_raw(raw);
    if info == GrB_Info::GrB_SUCCESS && !matrix.inner.is_null() {
        Some(Arc::new(matrix))
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub(super) struct SampledRelation {
    matrix: Arc<GraphblasMatrix>,
    converged: bool,
    source: Option<usize>,
    target: Option<usize>,
}

impl SampledRelation {
    fn nvals(&self) -> Option<usize> {
        let mut n = 0;
        (unsafe { GrB_Matrix_nvals(&mut n, self.matrix.inner) } == GrB_Info::GrB_SUCCESS).then_some(n as usize)
    }
}

pub(super) struct MatrixSampler {
    n: usize,
    vertices: Vec<usize>,
    labels: Vec<Option<SampledRelation>>,
    max_star_iterations: usize,
}

impl MatrixSampler {
    pub fn new(n: usize, label_matrices: Vec<GrB_Matrix>, fixed_vertices: &[usize], config: SamplingConfig) -> Self {
        let percent = config.percent.clamp(1, 100);
        let count = ((n as u128 * percent as u128).div_ceil(100)) as usize;
        let mut rng = StdRng::seed_from_u64(config.seed);
        let mut vertices = index::sample(&mut rng, n, count).into_vec();
        // Every label uses the same vertex order; named endpoints must be present.
        vertices.extend_from_slice(fixed_vertices);
        vertices.sort_unstable();
        vertices.dedup();
        let indices: Vec<u64> = vertices.iter().map(|&v| v as u64).collect();
        let mut labels = Vec::with_capacity(label_matrices.len());
        for matrix in label_matrices {
            let mut result = std::ptr::null_mut();
            let info = unsafe { LAGraph_RPQMatrix_sample_submatrix(&mut result, matrix, indices.as_ptr(), indices.len() as u64) };
            labels.push(sample_matrix(info, result).map(|matrix| SampledRelation {
                matrix,
                converged: true,
                source: None,
                target: None,
            }));
        }
        Self {
            n,
            vertices,
            labels,
            max_star_iterations: config.max_star_iterations,
        }
    }

    pub fn label(&self, id: usize) -> Option<SampledRelation> {
        self.labels.get(id)?.clone()
    }

    pub fn vertex(&self, vertex: usize) -> Option<SampledRelation> {
        let local = self.vertices.binary_search(&vertex).ok()?;
        let mut matrix = std::ptr::null_mut();
        let info = unsafe { LAGraph_RPQMatrix_label(&mut matrix, local as u64, self.vertices.len() as u64, self.vertices.len() as u64) };
        Some(SampledRelation {
            matrix: sample_matrix(info, matrix)?,
            converged: true,
            source: Some(vertex),
            target: Some(vertex),
        })
    }

    pub fn seq(&self, lhs: Option<&SampledRelation>, rhs: Option<&SampledRelation>) -> Option<SampledRelation> {
        let (lhs, rhs) = (lhs?, rhs?);
        let mut matrix = std::ptr::null_mut();
        let info = unsafe { LAGraph_RPQMatrix_sample_apply(&mut matrix, lhs.matrix.inner, rhs.matrix.inner) };
        Some(SampledRelation {
            matrix: sample_matrix(info, matrix)?,
            converged: lhs.converged && rhs.converged,
            source: lhs.source,
            target: rhs.target,
        })
    }

    pub fn alt(&self, lhs: Option<&SampledRelation>, rhs: Option<&SampledRelation>) -> Option<SampledRelation> {
        let (lhs, rhs) = (lhs?, rhs?);
        let mut matrix = std::ptr::null_mut();
        let info = unsafe { LAGraph_RPQMatrix_sample_union(&mut matrix, lhs.matrix.inner, rhs.matrix.inner) };
        Some(SampledRelation {
            matrix: sample_matrix(info, matrix)?,
            converged: lhs.converged && rhs.converged,
            source: if lhs.source == rhs.source { lhs.source } else { None },
            target: if lhs.target == rhs.target { lhs.target } else { None },
        })
    }

    pub fn star(&self, body: Option<&SampledRelation>) -> Option<SampledRelation> {
        let body = body?;
        let mut matrix = std::ptr::null_mut();
        let info = unsafe { LAGraph_RPQMatrix_sample_identity(&mut matrix, self.vertices.len() as u64) };
        let identity = SampledRelation {
            matrix: sample_matrix(info, matrix)?,
            converged: true,
            source: None,
            target: None,
        };
        self.closure(Some(body), Some(&identity), false)
    }

    pub fn closure(&self, body: Option<&SampledRelation>, seed: Option<&SampledRelation>, left: bool) -> Option<SampledRelation> {
        let (body, seed) = (body?, seed?);
        // Operations allocate new matrices, so the immutable seed can be shared.
        let mut result = seed.clone();
        result.converged &= body.converged;
        let mut previous = result.nvals()?;
        for _ in 0..self.max_star_iterations {
            let expanded = if left {
                self.seq(Some(body), Some(&result))?
            } else {
                self.seq(Some(&result), Some(body))?
            };
            let merged = self.alt(Some(&result), Some(&expanded))?;
            let nvals = merged.nvals()?;
            result = merged;
            if nvals == previous {
                return Some(result);
            }
            previous = nvals;
        }
        result.converged = false;
        Some(result)
    }

    pub fn estimate(&self, relation: Option<&SampledRelation>) -> SampleEstimate {
        let Some(relation) = relation else { return SampleEstimate::default() };
        let (mut nnz, mut rows, mut cols, mut diagonal) = (0, 0, 0, 0);
        if unsafe { LAGraph_RPQMatrix_sample_stats(&mut nnz, &mut rows, &mut cols, &mut diagonal, relation.matrix.inner) } != GrB_Info::GrB_SUCCESS {
            return SampleEstimate::default();
        }
        if self.vertices.is_empty() {
            return SampleEstimate { exact: self.n == 0, converged: relation.converged, ..SampleEstimate::default() };
        }
        let scale = self.n as f64 / self.vertices.len() as f64;
        let n = self.n as f64;
        // A diagonal pair contains one sampled vertex, an off-diagonal pair contains two.
        let diagonal_scale = if relation.source.is_some() || relation.target.is_some() { 1.0 } else { scale };
        let row_scale = if relation.source.is_some() { 1.0 } else { scale };
        let col_scale = if relation.target.is_some() { 1.0 } else { scale };
        SampleEstimate {
            nnz: ((nnz - diagonal) as f64 * row_scale * col_scale + diagonal as f64 * diagonal_scale).min(n * n),
            rows: (rows as f64 * row_scale).min(n),
            cols: (cols as f64 * col_scale).min(n),
            exact: self.vertices.len() == self.n && relation.converged,
            converged: relation.converged,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{graph::GraphDecomposition, utils::build_graph};

    #[test]
    fn induced_subgraph_only_contains_selected_vertices() {
        let graph = build_graph(&[("a", "b", "p"), ("b", "c", "p")]);
        let matrix = graph.get_graph("p").unwrap().matrix();
        let a = graph.get_node_id("a").unwrap();
        let c = graph.get_node_id("c").unwrap();
        let sampler = MatrixSampler::new(graph.num_nodes(), vec![matrix], &[a, c], SamplingConfig { percent: 1, seed: 0, max_star_iterations: 4 });
        assert!(sampler.vertices.contains(&a));
        assert!(sampler.vertices.contains(&c));
        let b = graph.get_node_id("b").unwrap();
        let expected_edges = if sampler.vertices.contains(&b) { 2 } else { 0 };
        assert_eq!(sampler.label(0).unwrap().nvals(), Some(expected_edges));
        assert_eq!(sampler.seq(sampler.label(0).as_ref(), sampler.label(0).as_ref()).unwrap().nvals(), Some(usize::from(expected_edges == 2)));
    }

    #[test]
    fn fixed_source_is_not_scaled_as_a_random_row() {
        let graph = build_graph(&[("a", "b", "p"), ("c", "d", "q")]);
        let p = graph.get_graph("p").unwrap().matrix();
        let a = graph.get_node_id("a").unwrap();
        let b = graph.get_node_id("b").unwrap();
        let sampler = MatrixSampler::new(graph.num_nodes(), vec![p], &[a, b], SamplingConfig { percent: 25, seed: 0, max_star_iterations: 4 });
        let sampled = sampler.seq(sampler.vertex(a).as_ref(), sampler.label(0).as_ref());
        assert_eq!(sampler.estimate(sampled.as_ref()).rows, 1.0);
    }

    #[test]
    fn fixed_endpoints_do_not_scale_the_empty_path() {
        let graph = build_graph(&[("a", "b", "q"), ("c", "d", "p")]);
        let p = graph.get_graph("p").unwrap().matrix();
        let a = graph.get_node_id("a").unwrap();
        let sampler = MatrixSampler::new(graph.num_nodes(), vec![p], &[a], SamplingConfig { percent: 25, seed: 0, max_star_iterations: 4 });
        let star = sampler.star(sampler.label(0).as_ref());
        let fixed = sampler.seq(sampler.vertex(a).as_ref(), star.as_ref());
        assert_eq!(sampler.estimate(fixed.as_ref()).nnz, 1.0);
        let fixed_target = sampler.seq(star.as_ref(), sampler.vertex(a).as_ref());
        assert_eq!(sampler.estimate(fixed_target.as_ref()).nnz, 1.0);
        let both_fixed = sampler.seq(fixed.as_ref(), sampler.vertex(a).as_ref());
        assert_eq!(sampler.estimate(both_fixed.as_ref()).nnz, 1.0);
    }

    #[test]
    fn self_loops_are_scaled_once_per_vertex() {
        let names: Vec<String> = (0..100).map(|v| v.to_string()).collect();
        let edges: Vec<_> = names.iter().map(|v| (v.as_str(), v.as_str(), "p")).collect();
        let graph = build_graph(&edges);
        let p = graph.get_graph("p").unwrap().matrix();
        let sampler = MatrixSampler::new(graph.num_nodes(), vec![p], &[], SamplingConfig { percent: 1, seed: 0, max_star_iterations: 4 });
        let relation = sampler.seq(sampler.label(0).as_ref(), sampler.label(0).as_ref());
        assert_eq!(sampler.estimate(relation.as_ref()).nnz, 100.0);
        let star = sampler.star(sampler.label(0).as_ref());
        assert_eq!(sampler.estimate(star.as_ref()).nnz, 100.0);
    }

    #[test]
    fn iteration_limit_does_not_mark_partial_closure_as_exact() {
        let graph = build_graph(&[("a", "b", "p"), ("b", "c", "p")]);
        let p = graph.get_graph("p").unwrap().matrix();
        let sampler = MatrixSampler::new(graph.num_nodes(), vec![p], &[], SamplingConfig { percent: 100, seed: 0, max_star_iterations: 1 });
        let star = sampler.star(sampler.label(0).as_ref());
        let estimate = sampler.estimate(star.as_ref());
        assert!(!estimate.converged);
        assert!(!estimate.exact);
        let seq = sampler.seq(star.as_ref(), sampler.label(0).as_ref());
        assert!(!sampler.estimate(seq.as_ref()).converged);
    }
}
