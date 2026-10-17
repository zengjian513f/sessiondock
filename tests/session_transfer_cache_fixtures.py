"""Observe native file opens during transfer browser scenarios."""
# run_validation: skip
import ctypes
import os
import struct


class Opens:
    def __init__(self, paths, shared=None):
        libc=ctypes.CDLL(None,use_errno=True)
        self.shared=shared
        self.fd=shared.fd if shared else libc.inotify_init1(os.O_NONBLOCK|os.O_CLOEXEC)
        assert self.fd>=0
        self.names=shared.names if shared else {}
        self.pending=set()
        for path in paths:
            watch=libc.inotify_add_watch(self.fd,os.fsencode(path),0x20)
            assert watch>=0
            self.names[watch]=(self,path.name)
    def take(self):
        while True:
            try: data=os.read(self.fd,65536)
            except BlockingIOError:
                result=self.pending;self.pending=set();return result
            offset=0
            while offset<len(data):
                watch,mask,cookie,length=struct.unpack_from('iIII',data,offset)
                assert not mask&0x4000,'inotify queue overflow'
                if mask&0x20:
                    owner,name=self.names[watch];owner.pending.add(name)
                offset+=16+length
    def close(self):
        if not self.shared:os.close(self.fd)
