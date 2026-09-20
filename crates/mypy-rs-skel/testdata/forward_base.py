class Sub(Base):
    def read(self) -> int:
        return self.v


class Base:
    def __init__(self) -> None:
        self.v = 1
