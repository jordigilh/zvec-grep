def helper():
    pass


def typed_helper(value: Base) -> Base:
    return value


def test_child():
    helper()


def negative_call():
    missing()
