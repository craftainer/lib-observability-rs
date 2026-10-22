# template-rust-crate's implementation of template-base's release contract
# (docs/TEMPLATE.md, "Release: a Makefile contract"). release.yml sets
# RELEASE_VERSION / RELEASE_TAG / RELEASE_ARCH. A source crate is built once,
# on one arch. CARGO_PUBLISH (a repo variable; non-empty) turns `publish` on;
# unset skips it. Credentials: see scripts/release-publish.sh.

.PHONY: release-arches build sbom release-assets publish release-check finalize

# Crate name and version come from scripts/crate_info.py (the one place that
# reads Cargo.toml). An instance that renames the crate needs no change here;
# a virtual workspace sets CARGO_PACKAGE (or CRATE) itself. Each is looked up
# lazily, once, on first use, so `release-arches` never reads Cargo.toml; an
# exported-but-empty CRATE counts as unset, as in the script. A failed lookup
# leaves the value empty (the script's stderr says why) and REQUIRE_CRATE,
# the first line of every target that needs them, stops with an error.
CARGO_PACKAGE ?=
CRATE_INFO := python3 -I scripts/crate_info.py
ifeq ($(strip $(CRATE)),)
CRATE = $(eval CRATE := $$(shell CARGO_PACKAGE='$$(CARGO_PACKAGE)' $(CRATE_INFO) name))$(CRATE)
endif
VERSION = $(eval VERSION := $$(shell RELEASE_VERSION='$$(RELEASE_VERSION)' $(CRATE_INFO) version))$(VERSION)
ASSET = dist/$(CRATE)-$(VERSION)
REQUIRE_CRATE = $(if $(and $(CRATE),$(VERSION)),,$(error scripts/crate_info.py could not determine the crate name and version (see its message above); set CRATE/CARGO_PACKAGE))

# renovate: datasource=docker depName=anchore/syft
SYFT_VERSION := v1.52.0

release-arches:
	@echo '["amd64"]'

# `cargo package` verifies the crate by building the unpacked sources. The
# release version is stamped into Cargo.toml/Cargo.lock only for the duration
# (release-stamped.sh), so the checkout stays clean. The runner's rustup
# honours rust-toolchain.toml, installing the pinned toolchain first.
build:
	$(REQUIRE_CRATE)
	@if command -v rustup >/dev/null 2>&1; then rustup toolchain install --profile minimal --no-self-update; fi
	rm -rf dist
	mkdir -p dist
	scripts/release-stamped.sh cargo package --locked --allow-dirty $(if $(CARGO_PACKAGE),-p $(CARGO_PACKAGE))
	@set -e; target=$$($(CRATE_INFO) target-dir); \
	cp "$$target/package/$(CRATE)-$(VERSION).crate" "$(ASSET).crate"

# Syft over the unpacked crate: its Cargo.toml and Cargo.lock list the
# dependencies the published source resolves to.
sbom:
	$(REQUIRE_CRATE)
	rm -rf dist/.sbom-src
	mkdir -p dist/.sbom-src
	tar -xzf $(ASSET).crate -C dist/.sbom-src --strip-components=1
	docker run --rm -u "$$(id -u):$$(id -g)" --tmpfs /tmp:rw,mode=1777 -e HOME=/tmp -v "$(CURDIR)/dist:/dist" \
		anchore/syft:$(SYFT_VERSION) "dir:/dist/.sbom-src" \
		--source-name $(CRATE) --source-version $(VERSION) -o "spdx-json=/$(ASSET).spdx.json"
	rm -rf dist/.sbom-src

release-assets:
	$(REQUIRE_CRATE)
	@set -e; for f in '$(ASSET).crate' '$(ASSET).spdx.json'; do \
		test -f "$$f" || { echo "missing release asset: $$f" >&2; exit 1; }; \
	done; ls dist

# Credentials and the dry-run switch: scripts/release-publish.sh.
publish:
ifeq ($(CARGO_PUBLISH),)
	@echo "CARGO_PUBLISH not set -- skipping publish."
else
	$(REQUIRE_CRATE)
	CARGO_PACKAGE='$(CARGO_PACKAGE)' CRATE='$(CRATE)' scripts/release-stamped.sh scripts/release-publish.sh
endif

# Runs once after the arch leg, inside the devcontainer; dist/ holds the
# downloaded .crate. See scripts/release-check.sh.
release-check:
	$(REQUIRE_CRATE)
	CARGO_PACKAGE='$(CARGO_PACKAGE)' CRATE='$(CRATE)' scripts/release-check.sh

# Nothing to combine: a source crate is built once.
finalize:
	@echo "Single source crate -- nothing to finalize."
