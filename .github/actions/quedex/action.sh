#!/bin/bash
set -euo pipefail

# quedex GitHub Action execution script
# Installs quedex, runs the plan, and outputs results to GITHUB_STEP_SUMMARY

readonly STORE_DIR="${GITHUB_WORKSPACE}/.quedex"
readonly RUN_ID="gh-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}"

# Colors for output (disabled if not a tty)
if [[ -t 1 ]]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  NC='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' NC=''
fi

log_info() { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[SUCCESS]${NC} $*"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Install quedex CLI
install_quedex() {
  if [[ -n "${INPUT_VERSION}" ]]; then
    log_info "Installing quedex v${INPUT_VERSION} from crates.io..."
    cargo install quedex --version "${INPUT_VERSION}"
  else
    log_info "Building quedex from source..."
    cargo build --release
    export PATH="${GITHUB_WORKSPACE}/target/release:${PATH}"
  fi

  # Verify installation
  if ! command -v quedex &> /dev/null; then
    log_error "quedex installation failed"
    exit 1
  fi
  log_success "quedex installed: $(quedex --version)"
}

# Run quedex plan
run_quedex() {
  local plan="${INPUT_PLAN}"
  local fail_fast_flag=""

  if [[ "${INPUT_FAIL_FAST}" != "true" ]]; then
    fail_fast_flag="--no-fail-fast"
  fi

  log_info "Running plan: ${plan}"
  log_info "Run ID: ${RUN_ID}"
  log_info "Store: ${STORE_DIR}"

  local exit_code=0
  # shellcheck disable=SC2086
  quedex run "${plan}" \
    --store "${STORE_DIR}" \
    --run-id "${RUN_ID}" \
    ${fail_fast_flag:+"${fail_fast_flag}"} || exit_code=$?

  return ${exit_code}
}

# Parse state.json and extract results
parse_results() {
  local state_file="${STORE_DIR}/runs/${RUN_ID}/state.json"

  if [[ ! -f "${state_file}" ]]; then
    log_error "State file not found: ${state_file}"
    echo "status=Unknown" >> "${GITHUB_OUTPUT}"
    echo "duration=0" >> "${GITHUB_OUTPUT}"
    return 1
  fi

  # Extract status
  local status
  status=$(jq -r '.status' "${state_file}")
  echo "status=${status}" >> "${GITHUB_OUTPUT}"

  # Calculate duration (started_at and completed_at are RFC3339)
  local started_at completed_at duration
  started_at=$(jq -r '.started_at // empty' "${state_file}")
  completed_at=$(jq -r '.completed_at // empty' "${state_file}")

  if [[ -n "${started_at}" && -n "${completed_at}" ]]; then
    # Convert to epoch and calculate difference (cross-platform)
    local start_epoch end_epoch
    if date --version >/dev/null 2>&1; then
      # GNU date
      start_epoch=$(date -d "${started_at}" +%s 2>/dev/null || echo "0")
      end_epoch=$(date -d "${completed_at}" +%s 2>/dev/null || echo "0")
    else
      # BSD date (macOS)
      start_epoch=$(date -j -f "%Y-%m-%dT%H:%M:%S" "${started_at%%.*}" +%s 2>/dev/null || echo "0")
      end_epoch=$(date -j -f "%Y-%m-%dT%H:%M:%S" "${completed_at%%.*}" +%s 2>/dev/null || echo "0")
    fi
    duration=$((end_epoch - start_epoch))
  else
    duration=0
  fi
  echo "duration=${duration}" >> "${GITHUB_OUTPUT}"

  log_info "Status: ${status}"
  log_info "Duration: ${duration}s"
}

# Generate GITHUB_STEP_SUMMARY
generate_summary() {
  local state_file="${STORE_DIR}/runs/${RUN_ID}/state.json"

  if [[ ! -f "${state_file}" ]]; then
    log_warn "Cannot generate summary: state file not found"
    return
  fi

  local status plan_name total_tasks completed failed skipped
  status=$(jq -r '.status' "${state_file}")
  plan_name=$(jq -r '.run_name // "Unknown"' "${state_file}")

  # Count tasks by status
  total_tasks=$(jq '.tasks | length' "${state_file}")
  completed=$(jq '[.tasks[] | select(.status == "Succeeded")] | length' "${state_file}")
  failed=$(jq '[.tasks[] | select(.status == "Failed")] | length' "${state_file}")
  skipped=$(jq '[.tasks[] | select(.status == "Skipped")] | length' "${state_file}")

  # Status emoji
  local status_emoji
  case "${status}" in
    "Succeeded") status_emoji="✅" ;;
    "Failed") status_emoji="❌" ;;
    "Canceled") status_emoji="⚠️" ;;
    *) status_emoji="❓" ;;
  esac

  # Write summary
  {
    echo "## ${status_emoji} quedex Execution Summary"
    echo ""
    echo "| Property | Value |"
    echo "|----------|-------|"
    echo "| **Plan** | \`${plan_name}\` |"
    echo "| **Run ID** | \`${RUN_ID}\` |"
    echo "| **Status** | ${status} |"
    echo "| **Total Tasks** | ${total_tasks} |"
    echo "| **Completed** | ${completed} |"
    echo "| **Failed** | ${failed} |"
    echo "| **Skipped** | ${skipped} |"
    echo ""

    # List failed tasks if any
    if [[ "${failed}" -gt 0 ]]; then
      echo "### ❌ Failed Tasks"
      echo ""
      echo "| Task ID | Exit Code |"
      echo "|---------|-----------|"
      jq -r '.tasks[] | select(.status == "Failed") | "| `\(.id)` | \(.exit_code // "N/A") |"' "${state_file}"
      echo ""
    fi

    # Execution timeline
    echo "<details>"
    echo "<summary>📋 Task Execution Details</summary>"
    echo ""
    echo "| Task | Status | Exit Code |"
    echo "|------|--------|-----------|"
    jq -r '.tasks[] | "| `\(.id)` | \(.status) | \(.exit_code // "-") |"' "${state_file}"
    echo ""
    echo "</details>"
  } >> "${GITHUB_STEP_SUMMARY}"

  log_success "Summary written to GITHUB_STEP_SUMMARY"
}

# Main execution
main() {
  log_info "quedex GitHub Action starting..."

  # Validate inputs
  if [[ -z "${INPUT_PLAN}" ]]; then
    log_error "Input 'plan' is required"
    exit 1
  fi

  if [[ ! -f "${INPUT_PLAN}" ]]; then
    log_error "Plan file not found: ${INPUT_PLAN}"
    exit 1
  fi

  # Install quedex
  install_quedex

  # Run quedex and capture exit code
  local quedex_exit_code=0
  run_quedex || quedex_exit_code=$?

  # Parse results and set outputs
  parse_results || true

  # Generate summary
  generate_summary || true

  # Exit with quedex exit code for CI status
  if [[ ${quedex_exit_code} -ne 0 ]]; then
    log_error "quedex execution failed with exit code: ${quedex_exit_code}"
    exit ${quedex_exit_code}
  fi

  log_success "quedex execution completed successfully"
}

main "$@"
