# ======Construct API=====
class StructMixin:
    @classmethod
    def parse(cls, data: bytes) -> "StructMixin":
        ...
    
    def build(self) -> bytes:
        ...

class Int16ub:
    ...

class Int8ub:
    ...

class GreedyBytes:
    ...

def field(subcon):
    ...