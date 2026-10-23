help:  ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "\nNote: Make is only for specific functionality used by CI - run `just` for developer targets:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

always:

##@ Release Builds

release-all: release release-lean release-small ## all release builds

release: always ## the default build, big but pretty (builds in ~2min 35s)
	cargo build --release

release-lean: always ## lean and fast, with line renderer (builds in ~1min 30s)
	cargo build --release --no-default-features --features lean

release-small: always ## minimal dependencies, at cost of performance (builds in ~46s)
	cargo build --release --no-default-features --features small

##@ Debug Builds

debug: always ## the default build, big but pretty
	cargo build

debug-lean: always ## lean and fast, with line renderer
	cargo build --no-default-features --features lean

debug-small: always ## minimal dependencies, at cost of performance
	cargo build --no-default-features --features small

##@ Development

gix := $(shell cargo metadata --format-version 1 | jq -r .target_directory)/release/gix
$(gix): always
	cargo build --release --no-default-features --features small

##@ Testing

clear-cache: ## Remove persisted results of test-repositories, they regenerate automatically
	-find . -path "*fixtures/generated" -type d -exec rm -Rf \{\} \;

rust_repo = tests/fixtures/repos/rust.git
stress_git = bash $(abspath etc/scripts/stress.sh) git
$(rust_repo):
	mkdir -p $@
	cd $@ && $(stress_git) init --bare --object-format=sha1 && $(stress_git) remote add origin https://github.com/rust-lang/rust && $(stress_git) fetch

linux_repo = tests/fixtures/repos/linux.git
$(linux_repo):
	mkdir -p $@
	cd $@ && $(stress_git) init --bare --object-format=sha1 && $(stress_git) remote add origin https://github.com/torvalds/linux && $(stress_git) fetch

test_many_commits_1m_repo = tests/fixtures/repos/test-many-commits-1m.git
$(test_many_commits_1m_repo):
	mkdir -p $@
	cd $@ && $(stress_git) init --bare --object-format=sha1 && $(stress_git) remote add origin https://github.com/cirosantilli/test-many-commits-1m.git && $(stress_git) fetch

etc/gix-asciicast.svg:
	svg-term --cast=542159 --out $@ --window

## get all non-rc tags up to v5.8, oldest tag first (should have 78 tags)
## -> convert to commit ids
## -> write a new incremental commit-graph file for each commit id
tests/fixtures/commit-graphs/linux/long-chain: $(linux_repo)
	mkdir -p $@
	rm -rf $(linux_repo)/objects/info/*graph*
	set -ex && cd $(linux_repo) && \
		for tag in $$($(stress_git) tag --list --merged v5.8 --sort=version:refname | grep -Fv -- -rc); do \
			$(stress_git) show-ref -s "$$tag" | $(stress_git) commit-graph write --split=no-merge --stdin-commits; \
		done
	mv -f $(linux_repo)/objects/info/*graphs* $@
	actual=$$(ls -1 $@/commit-graphs/*.graph | wc -l); \
		if [ $$actual -ne 78 ]; then echo expected 78 commit-graph files, got $$actual; exit 1; fi

tests/fixtures/commit-graphs/linux/single-file: $(linux_repo)
	mkdir -p $@
	rm -rf $(linux_repo)/objects/info/*graph*
	cd $(linux_repo) && $(stress_git) show-ref -s v5.8 | $(stress_git) commit-graph write --stdin-commits
	mv -f $(linux_repo)/objects/info/*graph* $@

tests/fixtures/commit-graphs/rust/single-file: $(rust_repo)
	mkdir -p $@
	rm -rf $(rust_repo)/objects/info/*graph*
	cd $(rust_repo) && $(stress_git) fetch --tags && $(stress_git) show-ref -s 1.47.0 | $(stress_git) commit-graph write --stdin-commits
	mv -f $(rust_repo)/objects/info/*graph* $@

tests/fixtures/commit-graphs/test-many-commits-1m/single-file: $(test_many_commits_1m_repo)
	mkdir -p $@
	rm -rf $(test_many_commits_1m_repo)/objects/info/*graph*
	cd $(test_many_commits_1m_repo) \
		&& echo f4d21576c13d917e1464d9bc1323a560a5b8595d | $(stress_git) commit-graph write --stdin-commits
	mv -f $(test_many_commits_1m_repo)/objects/info/*graph* $@

commit_graphs = \
	tests/fixtures/commit-graphs/linux/long-chain \
	tests/fixtures/commit-graphs/linux/single-file \
	tests/fixtures/commit-graphs/rust/single-file \
	tests/fixtures/commit-graphs/test-many-commits-1m/single-file

##@ on CI

.PHONY: stress
stress: ## Run pack and commit-graph algorithms with SHA-1 and SHA-256 fixtures
	$(MAKE) -j3 $(linux_repo) $(rust_repo) release-lean
	bash etc/scripts/stress.sh packs "$(gix)" $(linux_repo) $(rust_repo)
	$(MAKE) stress-commitgraph
	$(MAKE) bench-gix-config

.PHONY: stress-commitgraph
stress-commitgraph: release-lean $(commit_graphs)
	bash etc/scripts/stress.sh commitgraph "$(gix)"
	set -x; for path in $(wordlist 2, 999, $^); do \
		time $(gix) --verbose --object-hash sha1 no-repo commit-graph verify $$path || exit; \
	done

.PHONY: bench-gix-config
bench-gix-config:
	cd gix-config && cargo bench

##@ Maintenance

baseline_asset_dir = gix/src/assets/baseline-init
baseline_asset_fixture = tests/fixtures/baseline-init

$(baseline_asset_fixture):
	mkdir -p $@
	cd "$@" && git init --bare && \
		sed -i '' -E '/bare = true|ignorecase = true|precomposeunicode = true|filemode = true/d' config && \
		sed -i '' 's/master/main/g' $$(find . -type f)

transport_fixtures = gix-transport/tests/fixtures
base_url = https://github.com/GitoxideLabs/gitoxide.git
update-curl-fixtures: ## use curl to fetch raw fixtures for use in unit test. Changes there might break them
	curl -D - -L "$(base_url)/info/refs?service=git-upload-pack"  > $(transport_fixtures)/v1/http-handshake.response
	curl -D - -H 'Git-Protocol: version=2' -L "$(base_url)/info/refs?service=git-upload-pack"  > $(transport_fixtures)/v2/http-handshake.response
	curl -H 'User-Agent: git/oxide-0.1.0' -D - -H 'Git-Protocol: version=1' -L "https://github.com/GitoxideLabs/nonexistent/info/refs?service=git-upload-pack"  > $(transport_fixtures)/http-401.response
	curl -D - -H 'Git-Protocol: version=1' -L "https://github.com/GitoxideLabs/gitoxide/info/refs?service=git-upload-pack"  > $(transport_fixtures)/http-404.response

update-assets: $(baseline_asset_fixture) ## refresh assets compiled into the binaries from their source
	-rm -Rf $(baseline_asset_dir)
	mkdir -p $(dir $(baseline_asset_dir))
	cp -R $< $(baseline_asset_dir)

force-update-assets: ## As update-assets, but will run git to update the baseline as well
	-rm -Rf $(baseline_asset_fixture)
	$(MAKE) update-assets
