use test_r::test;

test_r::enable!();

// TODO: reenable
//
// #[test]
// fn mems_shopping_cart_component() {
//     let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
//     let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
//
//     let state = AnalysisContext::new(component);
//     let mems = state.get_all_memories().unwrap();
//
//     pretty_assertions::assert_eq!(
//         mems,
//         vec![Mem {
//             mem_type: MemType {
//                 limits: Limits { min: 17, max: None }
//             }
//         }]
//     )
// }
//
// #[test]
// fn mems_shopping_cart_resource_component() {
//     let source_bytes = include_bytes!("../wasm/shopping-cart-resource.wasm");
//     let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
//
//     let state = AnalysisContext::new(component);
//     let mems = state.get_all_memories().unwrap();
//
//     pretty_assertions::assert_eq!(
//         mems,
//         vec![Mem {
//             mem_type: MemType {
//                 limits: Limits { min: 17, max: None }
//             }
//         }]
//     )
// }
//
// #[test]
// fn mems_file_service_component() {
//     let source_bytes = include_bytes!("../wasm/file-service.wasm");
//     let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
//
//     let state = AnalysisContext::new(component);
//     let mems = state.get_all_memories().unwrap();
//
//     pretty_assertions::assert_eq!(
//         mems,
//         vec![Mem {
//             mem_type: MemType {
//                 limits: Limits { min: 17, max: None }
//             }
//         }]
//     )
// }
//
// #[test]
// fn mems_auction_registry_composed_component() {
//     let source_bytes = include_bytes!("../wasm/auction_registry_composed.wasm");
//     let component: Component<IgnoreAllButMetadata> = Component::from_bytes(source_bytes).unwrap();
//
//     let state = AnalysisContext::new(component);
//     let mems = state.get_all_memories().unwrap();
//
//     pretty_assertions::assert_eq!(
//         mems,
//         vec![
//             Mem {
//                 mem_type: MemType {
//                     limits: Limits { min: 17, max: None }
//                 }
//             },
//             Mem {
//                 mem_type: MemType {
//                     limits: Limits { min: 17, max: None }
//                 }
//             },
//         ]
//     )
// }
