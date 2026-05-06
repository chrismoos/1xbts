
EXEC =./fourgv_iwf.exe
CC= g++
CCFLAGS = -O3 -Wno-deprecated -D__SUNOS4__




${EXEC}: fourgv_iwf.cc
	${CC} -static ${CCFLAGS} -o ${EXEC} ${OBJS} fourgv_iwf.cc packet.cc

clean:
	rm -f ${EXEC}
