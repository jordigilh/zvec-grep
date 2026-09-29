from .helper import Helper, make


def imported_user(value: Helper) -> Helper:
    return make(value)


def ambiguous_call():
    load()
