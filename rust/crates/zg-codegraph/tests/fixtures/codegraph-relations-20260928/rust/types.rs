trait Parent {}

trait Child: Parent {}

struct Impl;

struct Wrapper {
    value: Impl,
}

impl Parent for Impl {}
