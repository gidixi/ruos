# ruos boot smoke — exercises VFS, .wasm tools, FAT, network, pipes.
# Mounted onto the ISO as /etc/init.sh by `make run-test` (overrides the
# minimal init.sh used in normal `make iso` boots). The Makefile asserts
# stdout markers — keep the echo lines and command order in sync.
echo ruos boot OK
whoami
uname -a
uptime
mkdir /tmp/sm
ls /tmp
cp /etc/init.sh /tmp/sm/script
ls /tmp/sm
cat /tmp/sm/script
mv /tmp/sm/script /tmp/sm/renamed
ls /tmp/sm
head -n 2 /tmp/sm/renamed
tail -n 2 /tmp/sm/renamed
du -sh /tmp/sm
grep -n echo /tmp/sm/renamed
find /tmp -name *.sh
rm /tmp/sm/renamed
ls /tmp/sm
rm -r /tmp/sm
ls /tmp
free -h
df -h
lscpu
ps
lspci
ip
cat /mnt/hello.txt
echo --- FAT write test ---
cp /etc/init.sh /mnt/init.bak
cat /mnt/init.bak | head -n 3
echo --- ping test ---
ping -c 1 -W 2000 10.0.2.2
echo --- pipe smoke ---
ls /bin | wc -l
