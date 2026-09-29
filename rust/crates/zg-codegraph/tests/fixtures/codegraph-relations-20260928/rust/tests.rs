fn helper() {}

fn test_impl() {
    helper();
}

#[test]
fn attribute_test() {
    helper();
}

fn negative_call() {
    missing();
}
