"""Reproduce frozen catch annotations without consulting RadGuard output.

This is an optional maintenance tool. Normal quality checks use only the
checked-in JSON and Python's standard library, never a live DAD installation.
"""

import argparse
import gzip
import hashlib
import json
import pickle
import sys
from pathlib import Path

import quality_gate as gate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dadguard-source", type=Path, required=True)
    parser.add_argument("--dex", type=Path, required=True)
    parser.add_argument("--annotations", type=Path, required=True)
    args = parser.parse_args()
    payload = json.loads(args.annotations.read_text(encoding="utf-8"))
    dex_bytes = args.dex.read_bytes()
    if payload["dex_sha256"] != hashlib.sha256(dex_bytes).hexdigest():
        raise ValueError("annotation selection belongs to a different DEX")

    sys.path.insert(0, str(args.dadguard_source.resolve()))
    from dadguard.core.analysis.analysis import MethodAnalysis
    from dadguard.core.dex import DEX
    from dadguard.decompiler.decompile import DvMethod
    from dadguard.decompiler.writer import Writer

    original_visit = Writer.visit_catch_node

    def visit_catch(self, node):
        if not hasattr(self, "catch_descriptors"):
            self.catch_descriptors = []
        # MoveExceptionExpression.type preserves the instruction's exception
        # type even when propagation changes the type of its reusable variable.
        descriptor = node.exception_ins.type if node.exception_ins is not None else None
        self.catch_descriptors.append(descriptor)
        return original_visit(self, node)

    Writer.visit_catch_node = visit_catch
    fixture_root = Path(__file__).resolve().parent / "fixtures"
    cache = fixture_root / "dad_reference_cache" / f"dad-{args.dex.stem}.pickle.gz"
    with gzip.open(cache, "rb") as stream:
        frozen = pickle.load(stream)["output"]
    _, raw_types = gate.method_metadata_oracles(dex_bytes)
    vm = DEX(dex_bytes)
    records = []
    for selection in payload["records"]:
        descriptor, index = selection["class_name"], selection["method_index"]
        methods = [m for m in vm.get_class(descriptor).get_methods() if m.get_code() is not None]
        method = methods[index]
        signature = method.get_name() + method.get_descriptor().replace(" ", "")
        if signature != selection["method"]:
            raise ValueError(f"DEX method order has changed: {descriptor} {index}")
        frozen_lines = frozen[descriptor].splitlines()
        start, end = gate.method_sections(frozen_lines)[index]
        frozen_body = "\n".join(frozen_lines[start:end + 1])
        decompiled = DvMethod(MethodAnalysis(vm, method))
        decompiled.process()
        lines = decompiled.get_source().splitlines()
        sections = gate.method_sections(lines)
        if len(sections) != 1:
            raise ValueError(f"DAD did not render one method: {descriptor} {signature}")
        start, end = sections[0]
        if "\n".join(lines[start:end + 1]) != frozen_body:
            raise ValueError(f"DAD source differs from the frozen method: {descriptor} {signature}")
        record = {
            "class_name": descriptor,
            "method_index": index,
            "method": signature,
            "source_sha256": hashlib.sha256(frozen_body.encode()).hexdigest(),
            "catch_types": [gate.descriptor_to_java(t) if t else None for t in decompiled.writer.catch_descriptors],
        }
        # Independently require every annotated type to occur in the original
        # DEX handler table, and require exact frozen source/count agreement.
        gate.restore_dad_catch_annotations(frozen[descriptor], {index: record}, raw_types[descriptor])
        records.append(record)
    payload["records"] = records
    args.annotations.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"Reproduced {len(records)} frozen catch annotations")


if __name__ == "__main__":
    main()
