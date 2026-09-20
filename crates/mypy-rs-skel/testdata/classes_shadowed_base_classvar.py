class Base:
    def __init__(self) -> None:
        self.x = later()


class Sub(Base):
    x: str = "s"


def later() -> int:
    return 1
