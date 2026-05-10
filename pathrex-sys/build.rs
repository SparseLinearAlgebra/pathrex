//! Build script for pathrex-sys.
//!
//! Builds SuiteSparse:GraphBLAS and LAGraph from source as static libraries
//! and emits the link directives needed by the generated bindings.
//!
//! ## Source acquisition
//!
//! GraphBLAS is fetched at build time via `git clone --depth 1 --branch
//! <pin>` into `$OUT_DIR/graphblas-src/`. A sentinel file
//! `$OUT_DIR/graphblas-src/.pathrex-fetched` marks a completed clone so that
//! incremental rebuilds skip the network. The pinned tag lives in
//! [`GRAPHBLAS_TAG`].
//!
//! LAGraph is provided as a git submodule under `deps/LAGraph`
//!
//! ## docs.rs
//!
//! docs.rs sandboxes block all network access and have strict time/memory
//! limits. The `DOCS_RS` environment variable signals that we are running
//! under docs.rs; we skip the entire native build .

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/// Upstream SuiteSparse:GraphBLAS release we build against.
const GRAPHBLAS_REPO: &str = "https://github.com/DrTimothyAldenDavis/GraphBLAS.git";
const GRAPHBLAS_TAG: &str = "v10.3.1";

/// LAGraph submodule path, relative to this crate's manifest dir.
const LAGRAPH_REL_PATH: &str = "../deps/LAGraph";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DOCS_RS");

    if env::var_os("DOCS_RS").is_some() {
        eprintln!("pathrex-sys: detected DOCS_RS, skipping native build");
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by Cargo"));

    let lagraph_src = manifest_dir.join(LAGRAPH_REL_PATH);
    assert_lagraph_submodule_present(&lagraph_src);

    let graphblas_src = fetch_graphblas(&out_dir);
    let graphblas_install = build_graphblas_static(&graphblas_src);

    let lagraph_install = build_lagraph_static(&lagraph_src, &graphblas_install);

    emit_link_directives(&lagraph_install, &graphblas_install);

    // ---- Bindgen (only with `regenerate-bindings` feature) ----
    #[cfg(feature = "regenerate-bindings")]
    regenerate_bindings(&graphblas_install);
}

/// Bail with a error message if `deps/LAGraph` is empty
fn assert_lagraph_submodule_present(lagraph_src: &Path) {
    let marker = lagraph_src.join("CMakeLists.txt");
    if !marker.exists() {
        panic!(
            "LAGraph submodule not initialized: {} does not exist.\n\
             Run: git submodule update --init --recursive",
            marker.display()
        );
    }
}


/// Clone SuiteSparse:GraphBLAS at [`GRAPHBLAS_TAG`] into
/// `$OUT_DIR/graphblas-src/`. Returns the path to the source tree.
///
/// A sentinel file `<dir>/.pathrex-fetched` containing the pinned tag marks
/// a completed clone. If the sentinel matches the requested tag, the clone
/// is reused; if the tag has changed,
/// the entire directory is removed and re-cloned
fn fetch_graphblas(out_dir: &Path) -> PathBuf {
    let src_dir = out_dir.join("graphblas-src");
    let sentinel = src_dir.join(".pathrex-fetched");

    if let Ok(contents) = std::fs::read_to_string(&sentinel) {
        if contents.trim() == GRAPHBLAS_TAG {
            return src_dir;
        }
        eprintln!(
            "pathrex-sys: GraphBLAS pin changed (was '{}', want '{}'), re-fetching",
            contents.trim(),
            GRAPHBLAS_TAG
        );
        std::fs::remove_dir_all(&src_dir).unwrap_or_else(|e| {
            panic!(
                "failed to remove stale GraphBLAS clone at {}: {e}",
                src_dir.display()
            )
        });
    } else if src_dir.exists() {
        eprintln!(
            "pathrex-sys: incomplete GraphBLAS clone at {}, removing",
            src_dir.display()
        );
        std::fs::remove_dir_all(&src_dir).unwrap_or_else(|e| {
            panic!(
                "failed to remove incomplete GraphBLAS clone at {}: {e}",
                src_dir.display()
            )
        });
    }

    eprintln!(
        "pathrex-sys: cloning GraphBLAS {GRAPHBLAS_TAG} into {src_dir:#?}"
    );

    let status = Command::new("git")
        .args([
            "clone",
            "--depth=1",
            "--branch",
            GRAPHBLAS_TAG,
            GRAPHBLAS_REPO,
        ])
        .arg(&src_dir)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "failed to invoke `git clone` for GraphBLAS: {e}\n\
                 Is git installed and on PATH?"
            )
        });

    if !status.success() {
        panic!(
            "`git clone` of {GRAPHBLAS_REPO} (tag {GRAPHBLAS_TAG}) failed with status {status}.\n\
             If you are offline, pre-populate {} with a checked-out tree.",
            src_dir.display()
        );
    }

    std::fs::write(&sentinel, GRAPHBLAS_TAG).unwrap_or_else(|e| {
        panic!(
            "failed to write fetch sentinel at {}: {e}",
            sentinel.display()
        )
    });

    src_dir
}

/// Drive cmake against the fetched GraphBLAS source. Returns the install
/// prefix (i.e. the cmake-rs `dst` directory) — this is where
/// `lib{,64}/libgraphblas.a` and `lib{,64}/cmake/GraphBLAS/GraphBLASConfig.cmake`
/// live afterwards.
fn build_graphblas_static(graphblas_src: &Path) -> PathBuf {
    cmake::Config::new(graphblas_src)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_STATIC_LIBS", "ON")
        .define("GRAPHBLAS_BUILD_STATIC_LIBS", "ON")
        .define("GRAPHBLAS_USE_JIT", "OFF")
        .define("GRAPHBLAS_COMPACT", "OFF")
        .define("GRAPHBLAS_USE_OPENMP", "ON")
        .define("GRAPHBLAS_USE_CUDA", "OFF")
        .define("SUITESPARSE_DEMOS", "OFF")
        .define("BUILD_TESTING", "OFF")
        .profile("Release")
        .build()
}

/// Drive cmake against the `deps/LAGraph` submodule. Returns the install
/// prefix containing `lib{,64}/{liblagraph.a,liblagraphx.a}`.
fn build_lagraph_static(lagraph_src: &Path, graphblas_install: &Path) -> PathBuf {
    cmake::Config::new(lagraph_src)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_STATIC_LIBS", "ON")
        .define("BUILD_TESTING", "OFF")
        .define("CMAKE_PREFIX_PATH", graphblas_install)
        .define("GRAPHBLAS_ROOT", graphblas_install)
        .define("LAGRAPH_USE_OPENMP", "ON")
        .profile("Release")
        .build()
}

/// Emit `cargo:rustc-link-*` directives for both the static archives we just
/// built and the runtime libraries they depend on (OpenMP, libm, libdl).
fn emit_link_directives(lagraph_install: &Path, graphblas_install: &Path) {
    let lagraph_libdir = pick_libdir(lagraph_install, "liblagraph.a");
    let graphblas_libdir = pick_libdir(graphblas_install, "libgraphblas.a");

    println!(
        "cargo:rustc-link-search=native={}",
        lagraph_libdir.display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        graphblas_libdir.display()
    );

    // Order: lagraphx -> lagraph -> graphblas -> OS libs.
    println!("cargo:rustc-link-lib=static=lagraphx");
    println!("cargo:rustc-link-lib=static=lagraph");
    println!("cargo:rustc-link-lib=static=graphblas");

    link_runtime_libs();
}

fn pick_libdir(install_prefix: &Path, archive_name: &str) -> PathBuf {
    let candidates = [install_prefix.join("lib"), install_prefix.join("lib64")];
    candidates
        .iter()
        .find(|p| p.join(archive_name).exists())
        .cloned()
        .unwrap_or_else(|| {
            panic!("cmake build succeeded but {archive_name} not found in any of: {candidates:?}")
        })
}

/// Emit the OS-specific runtime libraries pulled in by the static
/// GraphBLAS archive: OpenMP runtime, libm, libdl, and (potentially)
/// libatomic.
fn link_runtime_libs() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "linux" => {
            // Default to gomp;
            // users on a pure-clang/libomp toolchain can override with RUSTFLAGS.
            println!("cargo:rustc-link-lib=dylib=gomp");
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=dl");
            println!("cargo:rustc-link-lib=dylib=m");
        }
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=omp");
            println!("cargo:rustc-link-lib=dylib=pthread");
        }
        "windows" => {
            // Nothing to emit here.
        }
        other => {
            eprintln!(
                "warning: pathrex-sys: unknown target OS '{other}', \
                 defaulting runtime libs to Linux conventions"
            );
            println!("cargo:rustc-link-lib=dylib=gomp");
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=dl");
            println!("cargo:rustc-link-lib=dylib=m");
        }
    }
}

#[cfg(feature = "regenerate-bindings")]
fn regenerate_bindings(graphblas_install: &Path) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lagraph_include = manifest_dir.join(LAGRAPH_REL_PATH).join("include");
    assert!(
        lagraph_include.join("LAGraph.h").exists(),
        "LAGraph.h not found at {}.\n\
         Fetch the submodule:\n  git submodule update --init --recursive",
        lagraph_include.display()
    );

    let graphblas_include = locate_graphblas_header(graphblas_install);

    let bindings = bindgen::Builder::default()
        .header(
            lagraph_include
                .join("LAGraph.h")
                .to_str()
                .expect("non-utf8 header path"),
        )
        .header(
            lagraph_include
                .join("LAGraphX.h")
                .to_str()
                .expect("non-utf8 header path"),
        )
        .clang_arg(format!("-I{}", graphblas_include.display()))
        .clang_arg(format!("-I{}", lagraph_include.display()))
        .allowlist_type("GrB_Index")
        .allowlist_type("GrB_Matrix")
        .allowlist_type("GrB_Vector")
        .allowlist_item("GrB_BOOL")
        .allowlist_item("GrB_LOR")
        .allowlist_item("GrB_LOR_LAND_SEMIRING_BOOL")
        .allowlist_item("GrB_Info")
        .allowlist_function("GrB_Matrix_new")
        .allowlist_function("GrB_Matrix_nvals")
        .allowlist_function("GrB_Matrix_dup")
        .allowlist_function("GrB_Matrix_free")
        .allowlist_function("GrB_Matrix_extractElement_BOOL")
        .allowlist_function("GrB_Matrix_build_BOOL")
        .allowlist_function("GrB_Vector_new")
        .allowlist_function("GrB_Vector_free")
        .allowlist_function("GrB_Vector_setElement_BOOL")
        .allowlist_function("GrB_Vector_nvals")
        .allowlist_function("GrB_Vector_extractTuples_BOOL")
        .allowlist_function("GrB_vxm")
        .allowlist_item("LAGRAPH_MSG_LEN")
        .allowlist_item("RPQMatrixOp")
        .allowlist_type("RPQMatrixPlan")
        .allowlist_type("LAGraph_Graph")
        .allowlist_type("LAGraph_Kind")
        .allowlist_function("LAGraph_CheckGraph")
        .allowlist_function("LAGraph_Init")
        .allowlist_function("LAGraph_Finalize")
        .allowlist_function("LAGraph_SetNumThreads")
        .allowlist_function("LAGraph_GetNumThreads")
        .allowlist_function("LAGraph_New")
        .allowlist_function("LAGraph_Delete")
        .allowlist_function("LAGraph_Cached_AT")
        .allowlist_function("LAGraph_MMRead")
        .allowlist_function("LAGraph_RPQMatrix")
        .allowlist_function("LAGraph_RPQMatrix_reduce")
        .allowlist_function("LAGraph_DestroyRpqMatrixPlan")
        .allowlist_function("LAGraph_RPQMatrix_label")
        .allowlist_function("LAGraph_RPQMatrix_Free")
        .allowlist_function("LAGraph_RegularPathQuery")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: false,
        })
        .derive_debug(true)
        .derive_copy(true)
        .layout_tests(false)
        .generate_comments(false)
        .generate()
        .expect("bindgen failed to generate bindings");

    bindings
        .write_to_file(manifest_dir.join("src/lagraph_sys_generated.rs"))
        .expect("failed to write bindgen output to src/lagraph_sys_generated.rs");
}

#[cfg(feature = "regenerate-bindings")]
fn locate_graphblas_header(install_prefix: &Path) -> PathBuf {
    let candidates = [
        install_prefix.join("include").join("suitesparse"),
        install_prefix.join("include"),
    ];
    candidates
        .iter()
        .find(|p| p.join("GraphBLAS.h").exists())
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "GraphBLAS.h not found in any of {candidates:?}.\n\
                 Did the GraphBLAS cmake install step run?"
            )
        })
}
