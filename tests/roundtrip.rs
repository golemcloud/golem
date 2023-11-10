use wasm_ast::component::Component;
use wasm_ast::DefaultAst;

#[test]
fn roundtrip_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<DefaultAst> = Component::from_bytes(source_bytes).unwrap();
    let result_bytes = component.into_bytes().unwrap();
    assert_eq!(source_bytes, result_bytes.as_slice());
}
