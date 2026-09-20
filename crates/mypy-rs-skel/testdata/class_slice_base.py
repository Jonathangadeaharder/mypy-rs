from typing import Generic, TypeVar

T = TypeVar("T")


class Shape:
    kind: str = "shape"

    def __init__(self, name: str) -> None:
        self.name = name

    def area(self) -> float:
        return 0.0

    def describe(self) -> str:
        return self.kind + ":" + self.name


class Sized(Generic[T]):
    def __init__(self, item: T) -> None:
        self.item = item

    def unwrap(self) -> T:
        return self.item

    def replace(self, item: T) -> "Sized[T]":
        return Sized(item)
