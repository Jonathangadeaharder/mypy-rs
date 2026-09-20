from typing import TypeVar

from class_slice_base import Shape, Sized

T = TypeVar("T")


class Circle(Shape):
    def __init__(self, radius: float) -> None:
        super().__init__("circle")
        self.radius = radius

    def area(self) -> float:
        return 3.14159 * self.radius * self.radius

    def describe(self) -> str:
        return super().describe() + "/r=" + str(self.radius)


class NamedBox(Sized[T]):
    def __init__(self, item: T, label: str) -> None:
        super().__init__(item)
        self.label = label

    def unwrap(self) -> T:
        return super().unwrap()


def combined_area(first: Shape, second: Shape) -> float:
    return first.area() + second.area()


def relabelled(box: Sized[int], label: str) -> NamedBox[int]:
    return NamedBox(box.unwrap(), label)


circle = Circle(2.0)
square = Shape("square")
area: float = combined_area(circle, square)
box = NamedBox[int](5, "five")
item: int = box.unwrap()
label: str = box.label
kind: str = Shape.kind
second: NamedBox[int] = relabelled(box, "six")
base_view: Shape = circle
view_area: float = base_view.area()
