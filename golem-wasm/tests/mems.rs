use golem_wasm::analysis::wit_parser::WitAnalysisContext;
use test_r::test;

test_r::enable!();

#[test]
fn mems_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");

    let state = WitAnalysisContext::new(source_bytes).unwrap();
    let mems = state.linear_memories();

    pretty_assertions::assert_eq!(
        mems,
        &[wasmparser::MemoryType {
            memory64: false,
            shared: false,
            initial: 17,
            maximum: None,
            page_size_log2: None,
        }]
    )
}

#[test]
fn mems_shopping_cart_resource_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart-resource.wasm");
    let state = WitAnalysisContext::new(source_bytes).unwrap();
    let mems = state.linear_memories();

    pretty_assertions::assert_eq!(
        mems,
        &[wasmparser::MemoryType {
            memory64: false,
            shared: false,
            initial: 17,
            maximum: None,
            page_size_log2: None,
        }]
    )
}

#[test]
fn mems_file_service_component() {
    let source_bytes = include_bytes!("../wasm/file-service.wasm");
    let state = WitAnalysisContext::new(source_bytes).unwrap();
    let mems = state.linear_memories();

    pretty_assertions::assert_eq!(
        mems,
        &[wasmparser::MemoryType {
            memory64: false,
            shared: false,
            initial: 17,
            maximum: None,
            page_size_log2: None,
        }]
    )
}

#[test]
fn mems_auction_registry_composed_component() {
    let source_bytes = include_bytes!("../wasm/auction_registry_composed.wasm");
    let state = WitAnalysisContext::new(source_bytes).unwrap();
    let mems = state.linear_memories();

    pretty_assertions::assert_eq!(
        mems,
        &[
            wasmparser::MemoryType {
                memory64: false,
                shared: false,
                initial: 17,
                maximum: None,
                page_size_log2: None,
            },
            wasmparser::MemoryType {
                memory64: false,
                shared: false,
                initial: 17,
                maximum: None,
                page_size_log2: None,
            }
        ]
    )
}
