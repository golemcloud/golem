use golem_wasm_ast::component::Component;
use golem_wasm_ast::DefaultAst;

#[test]
fn roundtrip_shopping_cart_component() {
    let source_bytes = include_bytes!("../wasm/shopping-cart.wasm");
    let component: Component<DefaultAst> = Component::from_bytes(source_bytes).unwrap();
    let result_bytes = component.clone().into_bytes().unwrap();
    let component2: Component<DefaultAst> = Component::from_bytes(&result_bytes).unwrap();

    assert_component_eq(component, component2);
}

fn assert_component_eq(a: Component<DefaultAst>, b: Component<DefaultAst>) {
    let bytes_a = a.into_bytes().unwrap();
    let bytes_b = b.into_bytes().unwrap();
    if bytes_a != bytes_b {
        let wat_a = wasmprinter::print_bytes(bytes_a).unwrap();
        let wat_b = wasmprinter::print_bytes(bytes_b).unwrap();

        let diff = colored_diff::PrettyDifference {
            expected: &wat_a,
            actual: &wat_b,
        };
        println!("{}", diff);

        panic!("Components are not equal");
    }
}
