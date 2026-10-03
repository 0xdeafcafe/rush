//go:build !darwin && !linux

package host

import "net"

func peerPID(net.Conn) int { return 0 }
