from typing import Any, Protocol
import math


class Worker(Protocol):
    def work(self, value: int) -> int:
        ...


class Alpha:
    def convert(self, value: int) -> int:
        return value + 1


class Beta:
    def convert(self, value: int) -> int:
        return value + 2


def leaf(value: int) -> int:
    return value + 3


def caller(value: int) -> int:
    return leaf(value)


def chain(value: int) -> int:
    return caller(value)


def trait_caller(worker: Worker, value: int) -> int:
    return worker.work(value)


def alpha_caller(receiver: Alpha, value: int) -> int:
    return receiver.convert(value)


def beta_caller(receiver: Beta, value: int) -> int:
    return receiver.convert(value)


def alias_caller(value: int) -> int:
    fn = leaf
    return fn(value)


def dynamic_caller(value: int) -> int:
    fn: Any = leaf
    return fn(value)


def external_caller(value: int) -> float:
    return math.sqrt(value)


from src.decoy import leaf as decoy_leaf
from src.helper import leaf as helper_leaf


def imported_alias_caller(value: int) -> int:
    return helper_leaf(value)


def decoy_alias_caller(value: int) -> int:
    return decoy_leaf(value)
