use std::env;
use std::path::{Path, PathBuf};

#[cfg(feature = "regenerate-bindings")]
use std::path::PathBuf as _BindgenPathBuf;

// LAGraph submodule path, relative to this crate's manifest dir.
const LAGRAPH_REL_PATH: &str = "../deps/LAGraph";

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lagraph_src = manifest_dir.join(LAGRAPH_REL_PATH);

    assert_lagraph_submodule_present(&lagraph_src);

    // Build LAGraph cmake and emit static link directives for
    // liblagraph.a + liblagraphx.a.
    build_lagraph_static(&lagraph_src);

    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-lib=dylib=graphblas");

    // System libraries needed by the static LAGraph archives.
    link_openmp_and_libm();

    // ---- Bindgen (only with `regenerate-bindings` feature) ----
    #[cfg(feature = "regenerate-bindings")]
    regenerate_bindings();

    println!("cargo:rerun-if-changed=build.rs");
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

/// Drive cmake against `deps/LAGraph`, producing `liblagraph.a` and
/// `liblagraphx.a` under `$OUT_DIR/lib` (or `$OUT_DIR/lib64`).
///
/// Static link order matters: `lagraphx` depends on symbols from `lagraph`'s
/// utility module (e.g. `LAGraph_New`, `LAGraph_MMRead`), so it must precede
/// `lagraph` in the linker invocation. `graphblas` is appended separately by
/// the caller.
fn build_lagraph_static(lagraph_src: &Path) {
    let dst = cmake::Config::new(lagraph_src)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("BUILD_STATIC_LIBS", "ON")
        .define("BUILD_TESTING", "OFF")
        .profile("Release")
        .define("SUITESPARSE_USE_OPENMP", "ON")
        .build();

    // cmake-rs installs into `dst/lib` on most systems but some distros
    // (Fedora, openSUSE, RHEL) use `lib64`. Try both.
    let candidates = [dst.join("lib"), dst.join("lib64")];
    let libdir = candidates
        .iter()
        .find(|p| p.join("liblagraph.a").exists())
        .unwrap_or_else(|| {
            panic!(
                "LAGraph build succeeded but liblagraph.a was not found in any of: {:?}",
                candidates
            )
        });

    println!("cargo:rustc-link-search=native={}", libdir.display());
    // Order: lagraphx → lagraph → graphblas (caller).
    println!("cargo:rustc-link-lib=static=lagraphx");
    println!("cargo:rustc-link-lib=static=lagraph");

    // Re-run the build script if the LAGraph sources or CMakeLists change.
    for sub in ["CMakeLists.txt", "src", "experimental", "include"] {
        println!("cargo:rerun-if-changed={}", lagraph_src.join(sub).display());
    }
}

/// Link OpenMP (`libgomp` on Linux with GCC, `libomp` on macOS Homebrew clang)
/// and libm
fn link_openmp_and_libm() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" => {
            // Default to gomp;
            // users on a pure-clang/libomp toolchain can override with RUSTFLAGS.
            println!("cargo:rustc-link-lib=dylib=gomp");
            println!("cargo:rustc-link-lib=dylib=m");
        }
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=omp");
        }
        "windows" => {
            // MSVC's OpenMP runtime is selected via /openmp at compile time
            // and linked implicitly. Nothing to emit here.
        }
        other => {
            // For BSDs and other targets, fall back to libgomp (most likely
            // available through gcc). Override with RUSTFLAGS if wrong.
            eprintln!(
                "warning: pathrex-sys: unknown target OS '{other}', \
                 defaulting OpenMP runtime to libgomp"
            );
            println!("cargo:rustc-link-lib=dylib=gomp");
            println!("cargo:rustc-link-lib=dylib=m");
        }
    }
}

#[cfg(feature = "regenerate-bindings")]
fn regenerate_bindings() {
    let manifest_dir = _BindgenPathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // LAGraph submodule is a sibling of the pathrex-sys crate dir.
    let lagraph_include = manifest_dir.join("../deps/LAGraph/include");
    assert!(
        lagraph_include.join("LAGraph.h").exists(),
        "LAGraph.h not found at {}.\n\
         Fetch the submodule:\n  git submodule update --init --recursive",
        lagraph_include.display()
    );

    let graphblas_include = [
        _BindgenPathBuf::from("/usr/local/include/suitesparse"),
        _BindgenPathBuf::from("/usr/include/suitesparse"),
    ]
    .into_iter()
    .find(|p| p.join("GraphBLAS.h").exists())
    .unwrap_or_else(|| {
        panic!(
            "GraphBLAS.h not found.\n\
             Install SuiteSparse:GraphBLAS so headers are in /usr/local/include/suitesparse."
        )
    });

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
        // Suppress C-language doc comments so rustdoc does not attempt to
        // compile them as Rust doctests.
        .generate_comments(false)
        .generate()
        .expect("bindgen failed to generate bindings");

    bindings
        .write_to_file(manifest_dir.join("src/lagraph_sys_generated.rs"))
        .expect("failed to write bindgen output to src/lagraph_sys_generated.rs");
}
