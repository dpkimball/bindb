# Resolve dpk-build from this Makefile's location (ignore ambient BUILD_ROOT pollution).
BUILD_ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../dpk-build)

include $(BUILD_ROOT)/Makefile.common
