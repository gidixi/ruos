/* hello-pthread — prova C/pthread su ruos (MT Fase 2 + poll_oneoff).
 *
 * Compilato con wasi-sdk per wasm32-wasip1-threads (vedi build-wasm.sh):
 * pthread_create arriva da wasi-libc e usa lo stesso ABI wasi-threads
 * (import "wasi" "thread-spawn" + export wasi_thread_start) gia' usato da
 * Rust. usleep esercita poll_oneoff (clock subscription).
 *
 * Output atteso: "PTHREAD_C_OK val=42 ret=123"
 */
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

static void *worker(void *arg)
{
    int *v = (int *)arg;
    usleep(20 * 1000); /* 20 ms dentro il thread: park del fiber, non del core */
    *v = 42;
    return (void *)123;
}

int main(void)
{
    int val = 0;
    pthread_t t;
    if (pthread_create(&t, NULL, worker, &val) != 0) {
        printf("PTHREAD_C_FAIL create\n");
        return 1;
    }
    void *ret = NULL;
    if (pthread_join(t, &ret) != 0) {
        printf("PTHREAD_C_FAIL join\n");
        return 1;
    }
    usleep(30 * 1000); /* 30 ms nel main */
    printf("PTHREAD_C_OK val=%d ret=%d\n", val, (int)(long)ret);
    return 0;
}
