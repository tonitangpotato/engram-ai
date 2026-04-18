#!/usr/bin/env python3
"""
Setup script for engram-ai

For development:
    pip install -e .
    pip install -e ".[sentence-transformers]"
    pip install -e ".[all]"
"""

from setuptools import setup

# All config is in pyproject.toml (PEP 621)
# This file exists for compatibility with older tools
setup()
