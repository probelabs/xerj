import argparse
import importlib.util
import json
import os
import pathlib
import tempfile
import unittest
from unittest import mock


HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "optimize_minilm_fp32", HERE / "optimize_minilm_fp32.py"
)
assert SPEC is not None and SPEC.loader is not None
RECIPE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RECIPE)


class RecipeTests(unittest.TestCase):
    @unittest.skipUnless(
        RECIPE.current_toolchain_matches_contract(),
        "live check requires the documented locked CPython environment",
    )
    def test_current_locked_toolchain_is_exact(self):
        self.assertEqual(RECIPE.verify_toolchain(), RECIPE.PACKAGE_VERSIONS)

    def test_complete_directory_is_published_at_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            staged = root / ".staged"
            staged.mkdir()
            (staged / RECIPE.MODEL_FILENAME).write_bytes(b"model")
            (staged / RECIPE.MANIFEST_FILENAME).write_bytes(b"manifest")
            destination = root / "published"

            RECIPE.publish_directory_no_replace(staged, destination)
            self.assertFalse(staged.exists())
            self.assertEqual(
                sorted(path.name for path in destination.iterdir()),
                sorted([RECIPE.MODEL_FILENAME, RECIPE.MANIFEST_FILENAME]),
            )

    def test_directory_publication_never_replaces_collision(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            staged = root / ".staged"
            staged.mkdir()
            (staged / "complete").write_text("candidate")
            destination = root / "published"
            destination.mkdir()
            (destination / "sentinel").write_text("original")

            with self.assertRaisesRegex(RECIPE.RecipeError, "refusing to overwrite"):
                RECIPE.publish_directory_no_replace(staged, destination)
            self.assertEqual((destination / "sentinel").read_text(), "original")
            self.assertEqual((staged / "complete").read_text(), "candidate")

    def test_exclusive_lock_records_owner_cleans_up_and_rejects_conflict(self):
        with tempfile.TemporaryDirectory() as directory:
            lock = pathlib.Path(directory) / "recipe.lock"
            with RECIPE.exclusive_lock(lock):
                self.assertEqual(lock.read_text(), f"pid={os.getpid()}\n")
                with self.assertRaises(FileExistsError):
                    with RECIPE.exclusive_lock(lock):
                        pass
            self.assertFalse(lock.exists())

    def test_lock_owner_does_not_remove_replacement_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            lock = pathlib.Path(directory) / "recipe.lock"
            with RECIPE.exclusive_lock(lock):
                lock.unlink()
                lock.write_text("replacement\n")
            self.assertEqual(lock.read_text(), "replacement\n")

    def test_unknown_source_is_rejected_by_bytes_before_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "model.onnx"
            source.write_bytes(b"not the model")
            with self.assertRaisesRegex(RECIPE.RecipeError, "byte length mismatch"):
                RECIPE.verify_file(
                    source,
                    label="source model",
                    expected_bytes=RECIPE.SOURCE_BYTES,
                    expected_sha256=RECIPE.SOURCE_SHA256,
                )

    def test_same_length_wrong_hash_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "model.onnx"
            source.write_bytes(b"same-size")
            with self.assertRaisesRegex(RECIPE.RecipeError, "SHA-256 mismatch"):
                RECIPE.verify_file(
                    source,
                    label="source model",
                    expected_bytes=len(b"same-size"),
                    expected_sha256="0" * 64,
                )

    def test_candidate_topology_must_match_exactly(self):
        valid = {
            "inputs": RECIPE.EXPECTED_INPUTS,
            "outputs": RECIPE.EXPECTED_OUTPUTS,
            "operators": RECIPE.EXPECTED_CANDIDATE_OPERATORS,
            "opsets": RECIPE.EXPECTED_OPSETS,
            "ir_version": RECIPE.EXPECTED_IR_VERSION,
            "nodes": 46,
            "initializer_types": {"FLOAT": 77},
            "uses_external_data": False,
        }
        RECIPE.verify_graph_contract(valid)
        for key in (
            "inputs",
            "outputs",
            "operators",
            "opsets",
            "ir_version",
            "nodes",
            "initializer_types",
            "uses_external_data",
        ):
            invalid = json.loads(json.dumps(valid))
            invalid[key] = None
            with self.subTest(key=key):
                with self.assertRaisesRegex(RECIPE.RecipeError, f"{key} mismatch"):
                    RECIPE.verify_graph_contract(invalid)

    def test_generated_manifest_embeds_full_checked_contract(self):
        source = {
            "bytes": RECIPE.SOURCE_BYTES,
            "sha256": RECIPE.SOURCE_SHA256,
            "inputs": RECIPE.EXPECTED_INPUTS,
            "outputs": RECIPE.EXPECTED_OUTPUTS,
        }
        candidate = dict(RECIPE.CONTRACT["graph"])
        candidate.update(
            {
                "bytes": RECIPE.CANDIDATE_BYTES,
                "sha256": RECIPE.CANDIDATE_SHA256,
            }
        )
        with tempfile.TemporaryDirectory() as directory:
            tokenizer = pathlib.Path(directory) / "tokenizer.json"
            tokenizer.write_bytes(b"")
            with tokenizer.open("r+b") as value:
                value.truncate(RECIPE.TOKENIZER_BYTES)
            with mock.patch.object(
                RECIPE, "sha256", return_value=RECIPE.TOKENIZER_SHA256
            ):
                generated = RECIPE.build_manifest(
                    source=source,
                    tokenizer=tokenizer,
                    candidate=candidate,
                    packages=RECIPE.PACKAGE_VERSIONS,
                )
        self.assertEqual(generated["artifact_contract"], RECIPE.CONTRACT)
        self.assertNotIn(str(tokenizer), json.dumps(generated))
        self.assertIn(
            "new prefix",
            generated["artifact_contract"]["xerj_contract"]["identity"],
        )

    def test_observed_package_drift_fails_through_manifest_projection(self):
        source = {
            "bytes": RECIPE.SOURCE_BYTES,
            "sha256": RECIPE.SOURCE_SHA256,
        }
        candidate = dict(RECIPE.CONTRACT["graph"])
        candidate.update(
            bytes=RECIPE.CANDIDATE_BYTES,
            sha256=RECIPE.CANDIDATE_SHA256,
        )
        packages = dict(RECIPE.PACKAGE_VERSIONS)
        packages["onnxruntime"] = "0.0.0"
        tokenizer = mock.Mock()
        tokenizer.stat.return_value.st_size = RECIPE.TOKENIZER_BYTES
        with mock.patch.object(
            RECIPE, "sha256", return_value=RECIPE.TOKENIZER_SHA256
        ):
            drifted = RECIPE.observed_contract(
                source=source,
                tokenizer=tokenizer,
                candidate=candidate,
                packages=packages,
            )
        with self.assertRaisesRegex(RECIPE.RecipeError, "does not exactly match"):
            RECIPE.verify_full_contract(drifted)

    def test_python_contract_drives_host_independent_toolchain_gate(self):
        contract = json.loads(json.dumps(RECIPE.CONTRACT))
        contract["optimizer"]["python"] = "CPython 9.8.7"
        with (
            mock.patch.object(RECIPE, "CONTRACT", contract),
            mock.patch.object(RECIPE.platform, "python_implementation", return_value="CPython"),
            mock.patch.object(RECIPE.sys, "version_info", (9, 8, 6)),
        ):
            with self.assertRaisesRegex(RECIPE.RecipeError, "CPython 9.8.7"):
                RECIPE.verify_toolchain()

    def test_optimizer_arguments_are_contract_driven_and_strictly_typed(self):
        contract = json.loads(json.dumps(RECIPE.CONTRACT))
        contract["optimizer"]["arguments"]["num_heads"] = 6
        arguments, external_data = RECIPE.optimizer_configuration(contract)
        self.assertEqual(arguments["num_heads"], 6)
        self.assertNotIn("use_external_data_format", arguments)
        self.assertFalse(external_data)

        contract["optimizer"]["arguments"]["num_heads"] = True
        with self.assertRaisesRegex(RECIPE.RecipeError, "must be int"):
            RECIPE.optimizer_configuration(contract)

        contract = json.loads(json.dumps(RECIPE.CONTRACT))
        contract["optimizer"]["arguments"]["unknown"] = 1
        with self.assertRaisesRegex(RECIPE.RecipeError, "keys mismatch"):
            RECIPE.optimizer_configuration(contract)

    def test_invalid_python_contract_is_rejected(self):
        contract = json.loads(json.dumps(RECIPE.CONTRACT))
        contract["optimizer"]["python"] = "python3"
        with self.assertRaisesRegex(RECIPE.RecipeError, "must have the form"):
            RECIPE.required_python(contract)

    def test_python_patch_version_is_part_of_toolchain_gate(self):
        with (
            mock.patch.object(RECIPE.platform, "python_implementation", return_value="CPython"),
            mock.patch.object(RECIPE.sys, "version_info", (3, 13, 4)),
        ):
            with self.assertRaisesRegex(RECIPE.RecipeError, "CPython 3.13.5"):
                RECIPE.verify_toolchain()

    def test_failure_before_publish_leaves_no_public_directory_or_lock(self):
        class FakeOptimized:
            @staticmethod
            def save_model_to_file(path, use_external_data_format):
                self.assertFalse(use_external_data_format)
                pathlib.Path(path).write_bytes(b"candidate")

        source_graph = {
            "inputs": RECIPE.EXPECTED_INPUTS,
            "outputs": RECIPE.EXPECTED_OUTPUTS,
        }
        operative_contract = json.loads(json.dumps(RECIPE.CONTRACT))
        operative_contract["optimizer"]["arguments"]["num_heads"] = 6
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / "source.onnx"
            tokenizer = root / "tokenizer.json"
            source.write_bytes(b"source")
            tokenizer.write_bytes(b"tokenizer")
            output = root / "published"
            args = argparse.Namespace(
                source=source,
                tokenizer=tokenizer,
                output_dir=output,
            )
            with (
                mock.patch.object(RECIPE, "CONTRACT", operative_contract),
                mock.patch.object(RECIPE, "verify_file"),
                mock.patch.object(
                    RECIPE,
                    "verify_toolchain",
                    return_value=RECIPE.PACKAGE_VERSIONS,
                ),
                mock.patch.object(
                    RECIPE,
                    "inspect_graph",
                    side_effect=[source_graph, dict(RECIPE.CONTRACT["graph"])],
                ),
                mock.patch.object(
                    RECIPE, "optimize_model", return_value=FakeOptimized()
                ) as optimize,
                mock.patch.object(RECIPE, "verify_graph_contract"),
                mock.patch.object(
                    RECIPE,
                    "build_manifest",
                    side_effect=RECIPE.RecipeError("injected contract failure"),
                ),
            ):
                with self.assertRaisesRegex(
                    RECIPE.RecipeError, "injected contract failure"
                ):
                    RECIPE.run(args)

            expected_arguments = dict(operative_contract["optimizer"]["arguments"])
            expected_arguments.pop("use_external_data_format")
            optimize.assert_called_once_with(str(source), **expected_arguments)
            self.assertFalse(output.exists())
            self.assertFalse((root / ".published.optimize.lock").exists())
            self.assertEqual(
                list(root.glob(".published.staging-*")),
                [],
            )


if __name__ == "__main__":
    unittest.main()
