"""
CtxOne integrations with third-party LLM runtimes.

Each submodule wraps the core `ctxone.Hub` client in a shape that a
specific host expects. These are optional and their host-library
dependencies are declared as extras in pyproject.toml — importing a
submodule for a host you don't have installed will raise a clear
error telling you which extra to install.

Available:
    - `ctxone.integrations.openwebui` — Open WebUI Tool + Filter
      (install with ``pip install "ctxone[openwebui]"``).
"""
