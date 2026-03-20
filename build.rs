#[cfg(feature = "regenerate-bindings")]
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-lib=dylib=graphblas");
    println!("cargo:rustc-link-search=native=/usr/local/lib");
    println!("cargo:rustc-link-lib=dylib=lagraph");
    println!("cargo:rustc-link-search=native=deps/LAGraph/build/src");
    println!("cargo:rustc-link-lib=dylib=lagraphx");
    println!("cargo:rustc-link-search=native=deps/LAGraph/build/experimental");

    // ---- Bindgen (only with `regenerate-bindings` feature) ----
    #[cfg(feature = "regenerate-bindings")]
    regenerate_bindings();

    println!("cargo:rerun-if-changed=build.rs");
}

#[cfg(feature = "regenerate-bindings")]
fn regenerate_bindings() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let lagraph_include = manifest_dir.join("deps/LAGraph/include");
    assert!(
        lagraph_include.join("LAGraph.h").exists(),
        "LAGraph.h not found at {}.\n\
         Fetch the submodule:\n  git submodule update --init --recursive",
        lagraph_include.display()
    );

    let graphblas_include = [
        PathBuf::from("/usr/local/include/suitesparse"),
        PathBuf::from("/usr/include/suitesparse"),
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
        .allowlist_function("GrB_Matrix_free")
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
        .allowlist_function("LAGraph_New")
        .allowlist_function("LAGraph_Delete")
        .allowlist_function("LAGraph_Cached_AT")
        .allowlist_function("LAGraph_MMRead")
        .allowlist_function("LAGraph_RPQMatrix")
        .allowlist_function("LAGraph_DestroyRpqMatrixPlan")
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
