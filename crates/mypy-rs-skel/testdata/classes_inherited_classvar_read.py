class Base:
    kind: str = "base"

class Sub(Base):
    pass

k: str = Sub.kind
