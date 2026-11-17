use crate::helper::Helper;
pub use crate::helper::make;

fn imported_user(value: Helper) {
    make(value);
}

fn ambiguous_call() {
    load();
}
