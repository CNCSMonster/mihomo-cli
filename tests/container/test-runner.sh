#!/bin/bash
# mihomo-cli 容器测试运行器
# 用法: ./test-runner.sh [选项] [测试名称...]
#
# 选项:
#   --list              列出所有可用测试
#   --all               运行所有测试（包括 REQUIRES 不为空的）
#   --tag=TAG           按标签过滤测试（可多次指定）
#   --matrix            矩阵测试（多镜像）
#   --image=IMAGE       指定镜像
#   --parallel          并行执行
#   --jobs=N            并行度
#   --config=FILE       指定配置文件
#   --dry-run           显示计划但不执行
#   --required          Docker 不可用或测试被跳过时返回失败
#   --self-test         验证 runner 元数据和运行时探测脚本（无需 Docker）

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CONFIG_FILE="${SCRIPT_DIR}/config.toml"
TESTS_DIR="${SCRIPT_DIR}/tests"
source "${SCRIPT_DIR}/container-runtime.sh"

# 默认值
DEFAULT_IMAGE="ubuntu:24.04"
MATRIX_IMAGES=()
PARALLEL=false
JOBS=1
LIST_ONLY=false
DRY_RUN=false
RUN_ALL=false
REQUIRED=false
SELF_TEST=false
TAG_FILTER=()
SELECTED_TESTS=()

# 解析 TOML 配置（简单版）
parse_config() {
    if [[ -f "$CONFIG_FILE" ]]; then
        DEFAULT_IMAGE=$(grep '^default_image' "$CONFIG_FILE" | sed 's/.*= *"\(.*\)"/\1/' || echo "ubuntu:24.04")
        # Parse only the matrix_images array, stopping at its closing bracket.
        MATRIX_IMAGES=$(sed -n '/^matrix_images *= *\[/,/^\]/p' "$CONFIG_FILE" | sed -n 's/.*"\([^"]*\)".*/\1/p')
    fi
}

# 检测环境
detect_environment() {
    HAS_DOCKER=false
    HAS_PRIVILEGED=false

    if container_runtime_detect; then
        HAS_DOCKER=true
        if ! container_runtime info --format '{{json .SecurityOptions}}' 2>/dev/null | grep -q 'rootless'; then
            HAS_PRIVILEGED=true
        fi
    fi
    
    # 自动检测并行度
    if [[ "$PARALLEL" == "auto" ]]; then
        local cpu_jobs=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)
        local mem_mb=$(free -m 2>/dev/null | awk '/^Mem:/{print $7}' || echo 2048)
        local mem_jobs=$((mem_mb / 512))
        JOBS=$((cpu_jobs < mem_jobs ? cpu_jobs : mem_jobs))
        JOBS=$((JOBS < 4 ? JOBS : 4))
    fi
}

runner_self_test() {
    bash -n "$0" "${SCRIPT_DIR}/container-runtime.sh"
    local failed=0
    local test_file
    for test_file in "${TESTS_DIR}"/*.test; do
        for field in NAME DESC SCRIPT TAGS; do
            if ! grep -q "^${field}=" "$test_file"; then
                echo "missing ${field} in ${test_file}" >&2
                failed=1
            fi
        done
        local script
        script=$(grep '^SCRIPT=' "$test_file" | cut -d= -f2-)
        local declared_name file_name
        declared_name=$(grep '^NAME=' "$test_file" | cut -d= -f2-)
        file_name=$(basename "$test_file" .test)
        if [[ "$declared_name" != "$file_name" ]]; then
            echo "NAME ${declared_name} does not match file name ${file_name}" >&2
            failed=1
        fi
        if [[ ! -f "${SCRIPT_DIR}/scripts/${script}" ]]; then
            echo "missing script ${script} referenced by ${test_file}" >&2
            failed=1
        fi
    done
    local isolated_args network_args
    isolated_args=$(container_test_run_args "" "test-smoke.sh")
    network_args=$(container_test_run_args "network" "test-network.sh")
    if ! grep -Fxq -- "--network" <<<"$isolated_args" || ! grep -Fxq -- "none" <<<"$isolated_args"; then
        echo "missing network isolation for tests without network requirements" >&2
        failed=1
    fi
    if grep -Fxq -- "--network" <<<"$network_args"; then
        echo "network-required tests must not be network-isolated" >&2
        failed=1
    fi
    for required_arg in "${SCRIPT_DIR}:/tests:ro" "http_proxy=" "https_proxy=" "HTTP_PROXY=" "HTTPS_PROXY=" "ALL_PROXY=" "all_proxy="; do
        if ! grep -Fxq -- "$required_arg" <<<"$isolated_args"; then
            echo "missing isolated runner argument: $required_arg" >&2
            failed=1
        fi
    done

    if [[ "$failed" -ne 0 ]]; then
        return 1
    fi
    echo "✅ container runner self-test passed"
}

skip_test() {
    local message="$1"
    if [[ "$REQUIRED" == "true" ]]; then
        echo "❌ ${message}"
        return 1  # 失败
    fi
    echo "⊘ ${message}"
    return 2  # 跳过
}

# 列出测试
list_tests() {
    echo "可用测试："
    echo ""

    for test_file in "${TESTS_DIR}"/*.test; do
        if [[ -f "$test_file" ]]; then
            local name=$(basename "$test_file" .test)
            local desc=$(grep '^DESC=' "$test_file" | cut -d= -f2- || echo "无描述")
            local requires=$(grep '^REQUIRES=' "$test_file" | cut -d= -f2- || echo "")
            local tags=$(grep '^TAGS=' "$test_file" | cut -d= -f2- || echo "")

            local status=""
            if [[ "$requires" == *"real-proxy"* ]]; then
                status="⊘ (需要: $requires)"
            elif [[ "$requires" == *"systemd"* ]]; then
                status="✅ (由 just test-systemd-contract 严格覆盖)"
            elif [[ "$requires" == *"privileged"* ]] && [[ "$HAS_PRIVILEGED" != "true" ]]; then
                status="⊘ (需要特权模式)"
            elif [[ -n "$requires" && "$RUN_ALL" != "true" ]]; then
                status="⊘ (需要: $requires; 使用 --all)"
            else
                status="✅"
            fi

            printf "  %-30s %s %s\n" "$name" "$desc" "$status"
            if [[ -n "$tags" ]]; then
                printf "    %-28s [%s]\n" "" "$tags"
            fi
        fi
    done

    echo ""
    echo "环境检测："
    echo "  宿主平台: $(uname -s)/$(uname -m)"
    echo "  Docker: $([ "$HAS_DOCKER" = "true" ] && echo "✅ $(container_runtime_description)" || echo "❌")"
    echo "  特权容器: $([ "$HAS_PRIVILEGED" = "true" ] && echo "✅" || echo "❌")"
    echo ""
    echo "配置："
    echo "  默认镜像: $DEFAULT_IMAGE"
    echo "  矩阵镜像: ${MATRIX_IMAGES[*]:-无}"
    echo ""
    echo "提示："
    echo "  默认跳过 REQUIRES 不为空的测试，使用 --all 运行所有测试"
    echo "  使用 --tag=<tag> 按标签过滤"
}

# glibc ABI 守卫：动态链接的测试产物必须能被运行时镜像的 glibc 加载。
# 背景：新发行版宿主（或新 glibc 容器）编译出的二进制进不了老镜像，矩阵曾因
# 宿主 glibc 2.39 vs ubuntu:22.04 的 2.35 全军覆没（`GLIBC_2.39 not found`）。
# 静态 musl 产物无 GLIBC 符号需求，直接通过。
# 用法: assert_artifact_glibc_compat <runtime_image> <built_image>
assert_artifact_glibc_compat() {
    local runtime_image="$1"
    local image="$2"
    local artifact="${PROJECT_ROOT}/target/container-test/mihomo-cli"

    if [[ ! -x "$artifact" ]]; then
        echo "❌ glibc 守卫: 测试产物不存在或不可执行: $artifact"
        return 1
    fi

    # 1) 产物最高 GLIBC 符号版本；无符号 → 静态（musl）产物，直接通过
    if ! command -v objdump >/dev/null 2>&1; then
        echo "⊘ glibc 守卫: 宿主缺少 objdump(binutils)，跳过校验"
        return 0
    fi
    local required
    required=$(objdump -T "$artifact" 2>/dev/null \
        | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)*' | sed 's/GLIBC_//' | sort -uV | tail -1 || true)
    if [[ -z "$required" ]]; then
        echo "✓ glibc 守卫: 静态产物（无 GLIBC 符号需求），可运行于任意镜像"
        return 0
    fi

    # 2) 运行时镜像自身的 glibc 版本
    local first_line img_glibc
    first_line=$(container_runtime run --rm "$image" ldd --version 2>&1 | head -1 || true)
    if echo "$first_line" | grep -qi "musl"; then
        echo "❌ glibc 守卫: 镜像 $runtime_image 是 musl libc，无法加载 glibc 动态产物（需 GLIBC_$required）"
        echo "   修复: 用 musl 静态产物（cargo zigbuild --target x86_64-unknown-linux-musl --release）"
        return 1
    fi
    img_glibc=$(echo "$first_line" | awk '{print $NF}')
    if [[ ! "$img_glibc" =~ ^[0-9]+\.[0-9]+$ ]]; then
        echo "⊘ glibc 守卫: 无法解析镜像 $runtime_image 的 glibc 版本（ldd 输出: $first_line），跳过校验"
        return 0
    fi

    # 3) 比较：产物最高需求不得高于镜像 glibc（sort -V 取大者）
    local worst
    worst=$(printf '%s\n%s\n' "$img_glibc" "$required" | sort -V | tail -1)
    if [[ "$worst" != "$img_glibc" ]]; then
        echo "❌ glibc 守卫: 产物需 GLIBC_$required，高于镜像 $runtime_image 的 glibc $img_glibc"
        echo "   该二进制在镜像内会报 'GLIBC_x.xx not found'，测试结果不可信。"
        echo "   修复: 容器内编译产物（--matrix 已自动 MIHOMO_ARTIFACTS=container）或改用 musl 静态产物"
        return 1
    fi
    echo "✓ glibc 守卫: 产物最高需 GLIBC_$required ≤ 镜像 $runtime_image 的 glibc $img_glibc"
    return 0
}

# 构建测试镜像
build_test_image() {
    local image_name="${1:-mihomo-cli-test}"
    local runtime_image="${2:-$DEFAULT_IMAGE}"
    local dockerfile="${SCRIPT_DIR}/Dockerfile.simple"
    
    if [[ ! -f "$dockerfile" ]]; then
        echo "❌ Dockerfile 不存在: $dockerfile"
        return 1
    fi
    
    echo "构建测试镜像: $image_name"
    
    # 获取项目根目录（tests/container 的上级）
    local project_root="$(cd "${SCRIPT_DIR}/../.." && pwd)"

    bash "${SCRIPT_DIR}/prepare-artifacts.sh"

    container_runtime build \
        --build-arg "RUNTIME_IMAGE=${runtime_image}" \
        -t "$image_name" \
        -f "$dockerfile" \
        "$project_root"
}

container_test_run_args() {
    local requires="$1"
    local script="$2"
    local -a args=(--rm)

    if [[ "$requires" == *"privileged"* ]]; then
        args+=(--privileged)
    fi
    if [[ "$requires" != *"network"* && "$requires" != *"external"* && "$requires" != *"real-proxy"* ]]; then
        args+=(--network none)
    fi

    args+=(
        --env http_proxy=
        --env https_proxy=
        --env HTTP_PROXY=
        --env HTTPS_PROXY=
        --env ALL_PROXY=
        --env all_proxy=
        -v "${SCRIPT_DIR}:/tests:ro"
        "/tests/scripts/${script}"
    )
    printf '%s\n' "${args[@]}"
}

# 运行单个测试
run_test() {
    local test_name="$1"
    local image="${2:-mihomo-cli-test}"
    local test_file="${TESTS_DIR}/${test_name}.test"

    if [[ ! -f "$test_file" ]]; then
        echo "❌ 测试不存在: $test_name"
        return 1
    fi

    local script=$(grep '^SCRIPT=' "$test_file" | cut -d= -f2- || echo "test-${test_name}.sh")
    local requires=$(grep '^REQUIRES=' "$test_file" | cut -d= -f2- || echo "")

    # 检查环境依赖
    if [[ "$requires" == *"systemd"* ]]; then
        skip_test "跳过 $test_name (由 just test-systemd-contract 覆盖)"
        return
    fi
    if [[ "$requires" == *"privileged"* ]] && [[ "$HAS_PRIVILEGED" != "true" ]]; then
        skip_test "跳过 $test_name (Docker daemon 不支持特权容器)"
        return
    fi
    # Other requirements represent external fixtures/capabilities and are
    # opt-in through --all.
    if [[ -n "$requires" ]] && [[ "$RUN_ALL" != "true" ]]; then
        skip_test "跳过 $test_name (需要: ${requires}，使用 --all 运行)"
        return
    fi

    echo "--- 运行测试: $test_name (镜像: $image) ---"

    local -a docker_args=()
    mapfile -t docker_args < <(container_test_run_args "$requires" "$script")

    if [[ "$DRY_RUN" == "true" ]]; then
        printf '[dry-run] docker run'
        printf ' %q' "${docker_args[@]}"
        printf ' %q\n' "$image"
        return 0
    fi

    container_runtime run "${docker_args[@]:0:${#docker_args[@]}-1}" \
        "$image" \
        "${docker_args[-1]}"
}

# 主函数
main() {
    parse_config

    # 解析参数
    while [[ $# -gt 0 ]]; do
        case "$1" in
            list|--list)
                LIST_ONLY=true
                shift
                ;;
            --all)
                RUN_ALL=true
                shift
                ;;
            --tag=*)
                TAG_FILTER+=("${1#*=}")
                shift
                ;;
            --matrix)
                MATRIX_MODE=true
                # 矩阵镜像 glibc 可能低于宿主机，强制容器内编译测试产物
                export MIHOMO_ARTIFACTS=container
                shift
                ;;
            --image=*)
                DEFAULT_IMAGE_OVERRIDE="${1#*=}"
                shift
                ;;
            --parallel)
                PARALLEL=true
                shift
                ;;
            --jobs=*)
                JOBS="${1#*=}"
                PARALLEL=true
                shift
                ;;
            --config=*)
                CONFIG_FILE="${1#*=}"
                shift
                ;;
            --dry-run)
                DRY_RUN=true
                shift
                ;;
            --required)
                REQUIRED=true
                shift
                ;;
            --self-test)
                SELF_TEST=true
                shift
                ;;
            -*)
                echo "未知选项: $1"
                exit 1
                ;;
            *)
                SELECTED_TESTS+=("$1")
                shift
                ;;
        esac
    done

    if [[ "$SELF_TEST" == "true" ]]; then
        runner_self_test
        exit 0
    fi

    detect_environment

    # Listing is useful for diagnostics even when Docker is stopped.
    if [[ "$LIST_ONLY" == "true" ]]; then
        list_tests
        exit 0
    fi

    # 检查 Docker
    if [[ "$HAS_DOCKER" != "true" ]]; then
        container_runtime_diagnostic
        if [[ "$REQUIRED" == "true" ]]; then
            exit 1
        fi
        echo "⊘ Docker 不可用，跳过容器测试"
        exit 0
    fi

    # 确定要运行的测试
    local tests_to_run=()
    if [[ ${#SELECTED_TESTS[@]} -gt 0 ]]; then
        tests_to_run=("${SELECTED_TESTS[@]}")
    else
        for test_file in "${TESTS_DIR}"/*.test; do
            if [[ -f "$test_file" ]]; then
                local name=$(basename "$test_file" .test)

                # 标签过滤
                if [[ ${#TAG_FILTER[@]} -gt 0 ]]; then
                    local tags=$(grep '^TAGS=' "$test_file" | cut -d= -f2- || echo "")
                    local matched=false
                    for tag in "${TAG_FILTER[@]}"; do
                        if echo ",$tags," | grep -q ",$tag,"; then
                            matched=true
                            break
                        fi
                    done
                    if [[ "$matched" != "true" ]]; then
                        continue
                    fi
                fi

                tests_to_run+=("$name")
            fi
        done
    fi
    
    # 运行测试
    local failed=0
    local passed=0
    local skipped=0
    # Each matrix entry is a complete image built from the same Linux builder
    # and a different runtime base distribution.
    local runtime_images=("$DEFAULT_IMAGE")

    if [[ "${MATRIX_MODE:-false}" == "true" ]]; then
        IFS=$'\n' read -r -d '' -a runtime_images <<< "$MATRIX_IMAGES" || true
    elif [[ -n "${DEFAULT_IMAGE_OVERRIDE:-}" ]]; then
        runtime_images=("$DEFAULT_IMAGE_OVERRIDE")
    fi

    local runtime_image
    for runtime_image in "${runtime_images[@]}"; do
        local image="mihomo-cli-test-$(echo "$runtime_image" | tr '/:' '--')"
        if [[ "$DRY_RUN" != "true" ]]; then
            if ! build_test_image "$image" "$runtime_image"; then
                echo "❌ 构建测试镜像失败: $runtime_image"
                failed=$((failed + 1))
                continue
            fi
            if ! assert_artifact_glibc_compat "$runtime_image" "$image"; then
                failed=$((failed + 1))
                continue
            fi
        fi
        echo ""
        echo "=========================================="
        echo "运行时镜像: $runtime_image ($image)"
        echo "=========================================="

        for test_name in "${tests_to_run[@]}"; do
            if run_test "$test_name" "$image"; then
                passed=$((passed + 1))
            else
                local exit_code=$?
                if [[ $exit_code -eq 1 ]]; then
                    failed=$((failed + 1))
                else
                    skipped=$((skipped + 1))
                fi
            fi
        done
    done

    echo ""
    echo "=========================================="
    echo "统计: $passed 通过, $skipped 跳过, $failed 失败"
    if [[ $failed -eq 0 ]] && [[ $skipped -eq 0 ]]; then
        echo "✅ 所有测试通过"
    elif [[ $failed -eq 0 ]]; then
        echo "⚠️  $skipped 个测试被跳过（资源不足）"
        if [[ "$REQUIRED" == "true" ]]; then
            exit 1
        fi
    else
        echo "❌ $failed 个测试失败"
        exit 1
    fi
}

main "$@"
