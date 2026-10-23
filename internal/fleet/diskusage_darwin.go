package fleet

import (
	"encoding/binary"
	"syscall"
	"unsafe"

	"golang.org/x/sys/unix"
)

// dirUsage is what's inside a folder, read with getattrlistbulk: each call
// hands back the name, type and allocated size of a folder's worth of
// entries, where statUsage makes a call for every file. A temp folder with
// a million files in it walks three times as fast.
func dirUsage(dir string) int64 { return dirUsagePaced(dir, nil) }

func dirUsagePaced(dir string, pace *diskPacer) int64 {
	buf := make([]byte, 128<<10)
	n, ok := bulkUsage(unix.AT_FDCWD, dir, dir, buf, pace)
	if !ok {
		return statUsagePaced(dir, pace) // a file system without it
	}
	return n
}

// attrList is struct attrlist.
type attrList struct {
	bitmapCount uint16
	_           uint16
	common      uint32
	vol         uint32
	dir         uint32
	file        uint32
	fork        uint32
}

const (
	attrCmnName     = 0x1
	attrCmnObjType  = 0x8
	attrCmnReturned = 0x80000000
	attrDirAlloc    = 0x8 // ATTR_DIR_ALLOCSIZE
	attrFileAlloc   = 0x4 // ATTR_FILE_ALLOCSIZE
	vDir            = 2   // VDIR
)

var usageAttrs = attrList{bitmapCount: unix.ATTR_BIT_MAP_COUNT,
	common: attrCmnReturned | attrCmnName | attrCmnObjType, dir: attrDirAlloc, file: attrFileAlloc}

// bulkUsage is dirUsage for one folder and those under it; ok is false
// when the folder's file system can't answer getattrlistbulk. buf is
// reused all the way down: a folder's entries are taken from it before
// the folders under it are walked.
func bulkUsage(parent int, name, dir string, buf []byte, pace *diskPacer) (n int64, ok bool) {
	pace.yield()
	stamp, t, hit := heavyHit(dir, pace)
	if hit {
		return t, true
	}
	// Opening by name from the parent's descriptor skips resolving the
	// whole path again, which was half the cost of a deep tree.
	fd, err := unix.Openat(parent, name, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_NOFOLLOW|unix.O_CLOEXEC, 0)
	if err != nil {
		return 0, true
	}
	var subs []string
	for {
		pace.yield()
		r, _, e := syscall.Syscall6(unix.SYS_GETATTRLISTBULK, uintptr(fd), uintptr(unsafe.Pointer(&usageAttrs)),
			uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)), 0, 0)
		if e == unix.EINTR {
			continue
		}
		if e != 0 {
			unix.Close(fd)
			if e == unix.ENOTSUP || e == unix.EINVAL {
				return 0, false
			}
			return n, true
		}
		if r == 0 {
			break
		}
		p := buf
		for range int(r) {
			size, sub, next, good := bulkEntry(p)
			if !good {
				break
			}
			n += size
			if sub != "" {
				subs = append(subs, sub)
			}
			p = next
		}
	}
	for _, s := range subs {
		m, _ := bulkUsage(fd, s, dir+"/"+s, buf, pace)
		n += m
	}
	unix.Close(fd)
	heavyStore(dir, stamp, n)
	return n, true
}

// bulkEntry reads one entry getattrlistbulk packed: its allocated size, its
// name when it's a folder, and what follows it. The attributes come in the
// order asked for, each only if the returned set says it's there.
func bulkEntry(p []byte) (size int64, sub string, next []byte, ok bool) {
	if len(p) < 24 {
		return 0, "", nil, false
	}
	l := int(binary.LittleEndian.Uint32(p))
	if l < 24 || l > len(p) {
		return 0, "", nil, false
	}
	ent, next := p[:l], p[l:]
	common := binary.LittleEndian.Uint32(ent[4:])
	dirSet := binary.LittleEndian.Uint32(ent[12:])
	fileSet := binary.LittleEndian.Uint32(ent[16:])
	o := 24
	var name []byte
	if common&attrCmnName != 0 {
		if o+8 > l {
			return 0, "", next, true
		}
		off := int(int32(binary.LittleEndian.Uint32(ent[o:])))
		nl := int(binary.LittleEndian.Uint32(ent[o+4:]))
		if s := o + off; nl > 0 && s >= 0 && s+nl <= l {
			name = ent[s : s+nl-1] // less its NUL
		}
		o += 8
	}
	var typ uint32
	if common&attrCmnObjType != 0 && o+4 <= l {
		typ = binary.LittleEndian.Uint32(ent[o:])
		o += 4
	}
	if dirSet&attrDirAlloc != 0 && o+8 <= l {
		size = int64(binary.LittleEndian.Uint64(ent[o:]))
		o += 8
	}
	if fileSet&attrFileAlloc != 0 && o+8 <= l {
		size = int64(binary.LittleEndian.Uint64(ent[o:]))
	}
	if typ == vDir && len(name) > 0 {
		sub = string(name)
	}
	return size, sub, next, true
}
