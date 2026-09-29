pub trait Worker {
    fn work(&self, value: i32) -> i32;
}

pub struct Impl;

impl Worker for Impl {
    fn work(&self, value: i32) -> i32 {
        value + 1
    }
}

pub fn leaf(value: i32) -> i32 {
    value + 2
}

pub fn caller(value: i32) -> i32 {
    leaf(value)
}

pub fn chain(value: i32) -> i32 {
    caller(value)
}

pub fn dyn_caller(value: i32) -> i32 {
    let worker: &dyn Worker = &Impl;
    worker.work(value)
}

pub fn alias_caller(value: i32) -> i32 {
    let function = leaf;
    function(value)
}

impl Impl {
    fn adjust(value: i32) -> i32 {
        value + 3
    }
}

pub fn inherent_caller(value: i32) -> i32 {
    Impl::adjust(value)
}

mod helper;
mod decoy;

use crate::decoy::leaf as decoy_leaf;
use crate::helper::leaf as helper_leaf;

pub fn imported_alias_caller(value: i32) -> i32 {
    helper_leaf(value)
}

pub fn qualified_import_caller(value: i32) -> i32 {
    crate::helper::leaf(value)
}

pub fn decoy_alias_caller(value: i32) -> i32 {
    decoy_leaf(value)
}
