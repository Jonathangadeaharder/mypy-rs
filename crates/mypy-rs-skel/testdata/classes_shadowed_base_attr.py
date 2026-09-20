class Base:
    def __init__(self) -> None:
        self.x = later()


class Sub(Base):
    def __init__(self) -> None:
        super().__init__()
        self.x = 1


def later() -> str:
    return "s"
