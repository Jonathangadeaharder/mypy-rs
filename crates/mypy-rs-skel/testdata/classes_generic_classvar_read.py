from typing import Generic, TypeVar
T = TypeVar("T")

class Box(Generic[T]):
    label: str = "box"

x: str = Box.label
