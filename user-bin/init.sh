# ruos boot script
echo ruos boot OK
# coreutils smoke (kernel wiring + .wasm tools). shell has no redirection
# yet, so we exercise commands that don't require it.
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
