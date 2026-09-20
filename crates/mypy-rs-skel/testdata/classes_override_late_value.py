class Base:
    def m(self) -> None:
        self.x = value


class Sub(Base):
    x: int = 1


value = "a"
