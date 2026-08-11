//go:build !linux

package updater

import "context"

// RecoverPendingApply is a no-op on platforms that use native manual handoff.
func RecoverPendingApply(context.Context, string) (bool, error) { return false, nil }
