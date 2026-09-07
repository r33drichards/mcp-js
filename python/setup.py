"""Package pre-generated bindings; native compilation is a separate build step."""

from pathlib import Path

from setuptools import Distribution, setup
from setuptools.command.bdist_wheel import bdist_wheel
from setuptools.command.build_py import build_py


class NativeDistribution(Distribution):
    def has_ext_modules(self):
        return True


class BuildPython(build_py):
    def run(self):
        package = Path(__file__).parent / "mcp_js"
        if not (package / "_bindings.py").is_file() or not any(
            package.glob("libmcp_v8_uniffi.*")
        ):
            raise RuntimeError(
                "Run scripts/prepare-python-package.py --library <shared-library> before packaging"
            )
        super().run()


class NativeWheel(bdist_wheel):
    def get_tag(self):
        _, _, platform = super().get_tag()
        return "py3", "none", platform


setup(
    distclass=NativeDistribution,
    cmdclass={"build_py": BuildPython, "bdist_wheel": NativeWheel},
)
