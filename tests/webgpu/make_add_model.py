"""Encode the fixed ONNX Add smoke graph (IR 8, opset 13) without dependencies.

The runtime must still parse/validate this model before any inference claim.
Field numbers follow ONNX ModelProto/GraphProto/NodeProto/ValueInfoProto.
"""
from pathlib import Path

def varint(value):
    out = bytearray()
    while value > 127:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)

def integer(field, value):
    return varint(field << 3) + varint(value)

def blob(field, value):
    value = value.encode() if isinstance(value, str) else value
    return varint((field << 3) | 2) + varint(len(value)) + value

def tensor(name):
    shape = blob(1, integer(1, 4))
    tensor_type = integer(1, 1) + blob(2, shape)  # FLOAT, shape [4]
    return blob(1, name) + blob(2, blob(1, tensor_type))

node = blob(1, 'A') + blob(1, 'B') + blob(2, 'Y') + blob(3, 'Add4') + blob(4, 'Add')
graph = blob(1, node) + blob(2, 'mcpjs_add_smoke') + blob(11, tensor('A')) + blob(11, tensor('B')) + blob(12, tensor('Y'))
model = integer(1, 8) + blob(2, 'mcp-js') + blob(7, graph) + blob(8, integer(2, 13))
Path(__file__).with_name('onnx_add4.onnx').write_bytes(model)
