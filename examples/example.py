from dataclasses import dataclass
from construct import Int8ub, GreedyBytes, StructMixin, field


@dataclass
class ModbusRTUMessage(StructMixin):
    address: int = field(Int8ub)
    function_code: int = field(Int8ub)
    data: bytes = field(GreedyBytes)


if __name__ == "__main__":
    # Example usage
    message = ModbusRTUMessage(address=1, function_code=3, data=b'\x00\x01\x00\x02')
    built_data = message.build()
    print(f"Built Data: {built_data}")

    parsed_message = ModbusRTUMessage.parse(built_data)
    print(f"Parsed Message: {parsed_message}")